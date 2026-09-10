#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Rule-based diff analysis over two step ranges of a chrome-trace.

Performs a rule-based diff on the chrome-trace JSON persisted by dashboardServer
(`AIProf_<pid>.json`): windows events by IterationStep name, aligns identical
kernels to compute Δduration, and also produces the folded-stack input required
by the pyroscope differential flamegraph.

Usage:
    python3 diff_analysis.py <result_dir> <analysisId1> <pid1> <start1> <end1>
                                          <analysisId2> <pid2> <start2> <end2>
Output JSON (stdout): {"data": "<json string of diff rows>",
                       "flamegraph": "<json string of pyroscope diff>"}
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import re
import sys
from collections import defaultdict
from typing import Any, Dict, List, Optional, Tuple

try:
    from flamegraph import FlameGraphConverter
except ImportError:  # pragma: no cover
    from .flamegraph import FlameGraphConverter  # type: ignore


def _find_trace_file(result_dir: str, analysis_id: str, pid: str) -> Optional[str]:
    """Locate the chrome-trace JSON for a given pid within a single capture.

    analysis.py::extract_pid derives the pid via
    ``base.split('_')[-1].split('.')[0]``, so files are named like
    ``AIProf_<pid>.json``. Perform the equivalent reverse lookup here.
    """
    task_dir = os.path.join(result_dir, analysis_id)
    if not os.path.isdir(task_dir):
        return None
    candidates: List[str] = []
    for path in glob.glob(os.path.join(task_dir, "*.json")):
        base = os.path.basename(path)
        if base in ("Analysis_Summary.json", "Analysis_Report.html"):
            continue
        if base.endswith("aggregation-kernel.json") or base.endswith("OFFSET.json"):
            continue
        stem = base.rsplit(".", 1)[0]
        tail = stem.split("_")[-1]
        if tail == str(pid):
            candidates.append(path)
    if not candidates:
        return None
    # If multiple files share the same pid, pick the largest (usually AIProf_<pid>.json).
    candidates.sort(key=lambda p: os.path.getsize(p), reverse=True)
    return candidates[0]


def _load_trace(path: str) -> Dict[str, Any]:
    with open(path, "r", encoding="utf-8") as f:
        return json.load(f)


def cut_diff_data(start_time: float, end_time: float, data: Dict[str, Any]) -> Tuple[Dict[str, Dict[str, Any]], str]:
    """Aggregate events within [start_time, end_time] by name and extract the
    python_function call stacks as folded-stack text for flamegraph generation.

    Semantics:
    - Events with no dur are skipped; out-of-range events are clipped to the window (inner_dur).
    - Events whose name starts with IterationStep are skipped (framework overhead).
    - python_function events are collected per tid, then the call chain is
      rebuilt via (Python id, Python parent id) and emitted as folded-stack lines
      ``python:{pid};{tid};{fn1};{fn2}... {sample}``.
    """
    statfunc: Dict[str, Dict[str, Any]] = {}
    calltraces: List[str] = []
    python_events_by_tid: Dict[Any, List[Dict[str, Any]]] = defaultdict(list)

    if not data:
        return statfunc, ""

    trace_events = data.get("traceEvents", []) or []
    total_count = 0
    for d in trace_events:
        if "dur" not in d:
            continue
        ts = d["ts"]
        dur = d["dur"]
        if ts + dur < start_time or ts > end_time:
            continue
        name = d.get("name", "")
        if re.match(r"^IterationStep*", name):
            continue

        inner_dur = dur
        if ts < start_time:
            inner_dur = inner_dur + ts - start_time
        elif ts + dur > end_time:
            inner_dur = end_time - ts

        if d.get("cat") == "python_function":
            python_events_by_tid[d.get("tid")].append(d)

        entry = statfunc.get(name)
        if entry is None:
            statfunc[name] = {
                "count": 1,
                "dur": inner_dur,
                "dur_percent": 0,
                "count_percent": 0,
            }
        else:
            entry["dur"] += inner_dur
            entry["count"] += 1
        total_count += 1

    window = (end_time - start_time) or 1
    total_count = total_count or 1
    for key, value in statfunc.items():
        value["dur_percent"] = round(value["dur"] / window, 4)
        value["count_percent"] = round(value["count"] / total_count, 4)

    for tid, events in python_events_by_tid.items():
        if not events:
            continue
        events_map = {
            e.get("args", {}).get("Python id"): e
            for e in events if e.get("args", {}).get("Python id") is not None
        }
        if not events_map:
            continue
        parent_ids = {
            e.get("args", {}).get("Python parent id")
            for e in events if e.get("args", {}).get("Python parent id") is not None
        }
        processed_ids = set()
        for event in events:
            args = event.get("args", {})
            python_id = args.get("Python id")
            if python_id is None:
                continue
            if python_id in parent_ids or python_id in processed_ids:
                continue
            current_stack: List[Dict[str, Any]] = []
            cur = event
            while cur:
                current_stack.append(cur)
                cur_args = cur.get("args", {})
                cur_python_id = cur_args.get("Python id")
                if cur_python_id is None:
                    break
                processed_ids.add(cur_python_id)
                parent_id = cur_args.get("Python parent id")
                cur = events_map.get(parent_id)
            if not current_stack:
                continue
            current_stack.reverse()
            pid = current_stack[0].get("pid", "unknown_pid")
            fn_names = [e.get("name", "unknown_function") for e in current_stack]
            duration_ns = current_stack[-1].get("dur", 0) or 0
            last_sample = duration_ns / 100000000.0
            calltraces.append("python:{};{};{} {}".format(pid, tid, ";".join(fn_names), last_sample))

    return statfunc, "\n".join(calltraces)


def diff_analysis(data1: Dict[str, Dict[str, Any]], data2: Dict[str, Dict[str, Any]],
                  calltraces1: str, calltraces2: str) -> Tuple[List[Dict[str, Any]], Dict[str, Any]]:
    """Diff two aggregated windows. data2 is consumed in place (.pop)."""
    data2 = dict(data2)  # defensive copy so an external dict is not clobbered
    diff_rows: List[Dict[str, Any]] = []
    for key, value in data1.items():
        if key not in data2:
            diff_rows.append({
                "name": key,
                "before_time": round(value["dur"], 2),
                "after_time": 0,
                "time_diff": round(value["dur"], 2),
                "before_time_perc": round(value["dur_percent"], 4),
                "after_time_perc": 0,
                "time_perc_diff": round(value["dur_percent"], 4),
                "before_count": value["count"],
                "after_count": 0,
                "count_diff": value["count"],
                "before_count_perc": round(value["count_percent"], 4),
                "after_count_perc": 0,
                "count_perc_diff": round(value["count_percent"], 4),
            })
            continue
        r = data2[key]
        diff_rows.append({
            "name": key,
            "before_time": round(value["dur"], 2),
            "after_time": round(r["dur"], 2),
            "time_diff": round(value["dur"] - r["dur"], 2),
            "before_time_perc": round(value["dur_percent"], 4),
            "after_time_perc": round(r["dur_percent"], 4),
            "time_perc_diff": round(value["dur_percent"] - r["dur_percent"], 4),
            "before_count": value["count"],
            "after_count": r["count"],
            "count_diff": value["count"] - r["count"],
            "before_count_perc": round(value["count_percent"], 4),
            "after_count_perc": round(r["count_percent"], 4),
            "count_perc_diff": round(value["count_percent"] - r["count_percent"], 4),
        })
        data2.pop(key)
    for key, value in data2.items():
        diff_rows.append({
            "name": key,
            "before_time": 0,
            "after_time": round(value["dur"], 2),
            "time_diff": round(-value["dur"], 2),
            "before_time_perc": 0,
            "after_time_perc": round(value["dur_percent"], 4),
            "time_perc_diff": round(-value["dur_percent"], 4),
            "before_count": 0,
            "after_count": value["count"],
            "count_diff": -value["count"],
            "before_count_perc": 0,
            "after_count_perc": round(value["count_percent"], 4),
            "count_perc_diff": round(-value["count_percent"], 4),
        })

    converter = FlameGraphConverter()
    pyroscope_diff_json = converter.convert_to_pyroscope_diff(calltraces1, calltraces2)
    return diff_rows, pyroscope_diff_json


def run_diff(result_dir: str,
             analysis_id1: str, pid1: str, start1: float, end1: float,
             analysis_id2: str, pid2: str, start2: float, end2: float) -> Dict[str, Any]:
    """Public entry point: invoked by dashboardServer via subprocess stdin/args."""
    path1 = _find_trace_file(result_dir, analysis_id1, pid1)
    if not path1:
        raise FileNotFoundError(f"trace not found: analysisId={analysis_id1} pid={pid1}")

    if analysis_id1 == analysis_id2 and pid1 == pid2:
        data = _load_trace(path1)
        d1, ct1 = cut_diff_data(start1, end1, data)
        d2, ct2 = cut_diff_data(start2, end2, data)
    else:
        path2 = _find_trace_file(result_dir, analysis_id2, pid2)
        if not path2:
            raise FileNotFoundError(f"trace not found: analysisId={analysis_id2} pid={pid2}")
        d1, ct1 = cut_diff_data(start1, end1, _load_trace(path1))
        d2, ct2 = cut_diff_data(start2, end2, _load_trace(path2))

    diff_rows, flamegraph_json = diff_analysis(d1, d2, ct1, ct2)
    return {
        "data": json.dumps(diff_rows, ensure_ascii=False),
        "flamegraph": json.dumps(flamegraph_json, ensure_ascii=False),
    }


def _main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(description="Rule-based diff analysis over chrome-trace")
    parser.add_argument("--result-dir", required=True)
    parser.add_argument("--task1", required=True, help='JSON: {analysisId, pid, step_start, step_end}')
    parser.add_argument("--task2", required=True, help='JSON: {analysisId, pid, step_start, step_end}')
    args = parser.parse_args(argv)

    t1 = json.loads(args.task1)
    t2 = json.loads(args.task2)
    try:
        out = run_diff(
            args.result_dir,
            t1["analysisId"], str(t1["pid"]), float(t1["step_start"]), float(t1["step_end"]),
            t2["analysisId"], str(t2["pid"]), float(t2["step_start"]), float(t2["step_end"]),
        )
        sys.stdout.write(json.dumps(out, ensure_ascii=False))
        return 0
    except Exception as e:
        sys.stderr.write(f"diff_analysis error: {e}\n")
        return 1


if __name__ == "__main__":
    sys.exit(_main(sys.argv[1:]))
