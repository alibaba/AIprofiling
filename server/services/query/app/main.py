# SPDX-License-Identifier: Apache-2.0
"""AIProf query service — REST for the UI.

Pulls metadata from the store and streams artifacts from the blob store.
Two artifact families are exposed:

* CPU: `folded.txt` (Brendan-Gregg folded stacks) — used to render the
  main flamegraph and hot-frame ranks.
* GPU: `torch-trace.json` (Chrome/Perfetto trace emitted by
  `torch.profiler.export_chrome_trace`) — served raw for the Perfetto
  viewer, plus a `/kernels` endpoint that aggregates per-kernel stats
  so the UI does not have to parse the trace itself.

Kept intentionally small: no auth, no cross-session aggregation, no
report generation. Higher-level features belong upstream in the BFF or
in a dedicated service.
"""

from __future__ import annotations

import json
from collections import Counter, defaultdict
from typing import Any, Dict, List, Optional, Tuple

from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse, PlainTextResponse, Response

from server.common.config import settings
from server.common.storage import BlobStore
from server.common.store import Store


app = FastAPI(title="aiprof-query", version="0.1.0")
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

store = Store(settings.db_url)
blobs = BlobStore(settings.blob_dir)


@app.get("/healthz")
def healthz():
    return {"ok": True, "service": "query"}


@app.get("/api/v1/aiprof/sessions")
def list_sessions(
    host: Optional[str] = None,
    workload: Optional[str] = None,
    kind: Optional[str] = None,
    limit: int = Query(default=50, ge=1, le=500),
):
    return {"items": store.list_sessions(host=host, workload=workload, kind=kind, limit=limit)}


@app.get("/api/v1/aiprof/sessions/{session_id}")
def session_detail(session_id: str):
    s = store.get_session(session_id)
    if not s:
        raise HTTPException(404, "session not found")
    # Include a cheap manifest of which artifact families are present so
    # the UI can enable/disable the timeline & kernel tabs without pulling
    # the full blobs.
    s = dict(s)
    s["artifacts"] = _list_artifacts(s["blob_key"])
    return s


_KNOWN_ARTIFACTS = ("folded.txt", "torch-trace.json", "timeline.json", "cupti/kernels.jsonl")


def _list_artifacts(blob_key: str) -> Dict[str, bool]:
    """Return {artifact_name: present} for the well-known files.

    We don't enumerate the whole tarball — this stays O(len(_KNOWN_ARTIFACTS))
    probes so a huge session doesn't slow the sessions list.
    """
    present: Dict[str, bool] = {}
    for name in _KNOWN_ARTIFACTS:
        present[name] = blobs.extract_member(blob_key, name) is not None
    return present


@app.get("/api/v1/aiprof/sessions/{session_id}/folded", response_class=PlainTextResponse)
def session_folded(session_id: str):
    s = store.get_session(session_id)
    if not s:
        raise HTTPException(404, "session not found")
    data = blobs.extract_member(s["blob_key"], "folded.txt")
    if data is None:
        raise HTTPException(404, "folded.txt not in blob")
    return PlainTextResponse(data.decode("utf-8", errors="replace"))


@app.post("/api/v1/aiprof/diff", response_class=PlainTextResponse)
def diff_sessions(a: str, b: str):
    """Very small diff — signed sample deltas per folded stack.

    Result format:
        <stack> <delta>
    where positive means "more in b than a".
    """
    sa = store.get_session(a); sb = store.get_session(b)
    if not sa or not sb:
        raise HTTPException(404, "session not found")
    ta = blobs.extract_member(sa["blob_key"], "folded.txt") or b""
    tb = blobs.extract_member(sb["blob_key"], "folded.txt") or b""
    ca = _folded_to_counter(ta.decode("utf-8", errors="replace"))
    cb = _folded_to_counter(tb.decode("utf-8", errors="replace"))
    keys = sorted(set(ca) | set(cb))
    lines: List[str] = []
    for k in keys:
        d = cb.get(k, 0) - ca.get(k, 0)
        if d:
            lines.append(f"{k} {d}")
    return PlainTextResponse("\n".join(lines))


def _folded_to_counter(text: str) -> Counter:
    c: Counter = Counter()
    for line in text.splitlines():
        line = line.rstrip()
        if not line:
            continue
        parts = line.rsplit(" ", 1)
        if len(parts) != 2:
            continue
        stack, count = parts
        try:
            c[stack] = int(count)
        except ValueError:
            continue
    return c


# ---------------------------------------------------------------------------
# GPU artifacts
# ---------------------------------------------------------------------------

# Torch-emitted chrome-trace categories that represent GPU-side work.
_GPU_KERNEL_CATS = frozenset(("kernel", "gpu_memcpy", "gpu_memset"))


@app.get("/api/v1/aiprof/sessions/{session_id}/timeline")
def session_timeline(session_id: str):
    """Return the raw Chrome/Perfetto trace for this session, if any.

    The tarball may hold two flavours:
      * `torch-trace.json` — written by the Python agent's `--gpu` mode
        (torch.profiler.export_chrome_trace)
      * `timeline.json` — written by the Rust `py-cuda-tracer` plugin
        (fused perf + CUPTI kernel stream)

    We prefer the torch one when both exist because it carries richer
    per-kernel args. 404 when neither is present so the UI can fall
    back gracefully.
    """
    s = store.get_session(session_id)
    if not s:
        raise HTTPException(404, "session not found")
    for candidate in ("torch-trace.json", "timeline.json"):
        data = blobs.extract_member(s["blob_key"], candidate)
        if data:
            return Response(content=data, media_type="application/json")
    raise HTTPException(404, "no timeline artifact in session")


@app.get("/api/v1/aiprof/sessions/{session_id}/kernels")
def session_kernels(session_id: str):
    """Per-kernel aggregation extracted from the session's chrome trace.

    Output matches the shape the UI's `KernelBoard` component expects
    (`ui/webapp/src/pages/AIProf/result/index.tsx`):

        [{ "name": str, "total_ms": float, "count": int, "sm_util": float | null }, ...]

    Empty list on CPU-only captures — that is not an error condition.
    """
    s = store.get_session(session_id)
    if not s:
        raise HTTPException(404, "session not found")
    kernels = _extract_kernels(s["blob_key"])
    return JSONResponse(content={"items": kernels})


def _extract_kernels(blob_key: str) -> List[Dict[str, Any]]:
    """Aggregate per-kernel stats from a chrome trace inside a tarball.

    We stream the trace as text and parse it with json.loads once; sessions
    are small enough (< 100 MiB in practice) that a streaming parser is
    unnecessary. If neither timeline flavour is present, return [].
    """
    raw = blobs.extract_member(blob_key, "torch-trace.json")
    if raw is None:
        raw = blobs.extract_member(blob_key, "timeline.json")
    if raw is None:
        return []
    try:
        doc = json.loads(raw)
    except json.JSONDecodeError:
        return []
    events = doc.get("traceEvents") or []

    total_us: Dict[str, float] = defaultdict(float)
    count: Dict[str, int] = defaultdict(int)
    sm_sum: Dict[str, float] = defaultdict(float)
    sm_hits: Dict[str, int] = defaultdict(int)

    for ev in events:
        if ev.get("ph") != "X":
            continue
        cat = ev.get("cat") or ""
        if cat not in _GPU_KERNEL_CATS:
            continue
        dur = ev.get("dur")
        try:
            dur = float(dur)
        except (TypeError, ValueError):
            continue
        if dur <= 0:
            continue
        name = ev.get("name") or "?"
        total_us[name] += dur
        count[name] += 1
        # torch.profiler stores SM utilisation in args under this key
        # (see `torch.autograd.profiler.KinetoEvent`); the dashboard's
        # analysis.py uses the same key.
        args = ev.get("args") or {}
        occ = args.get("est. achieved occupancy %")
        if isinstance(occ, (int, float)):
            sm_sum[name] += float(occ)
            sm_hits[name] += 1

    out: List[Dict[str, Any]] = []
    for name, us in total_us.items():
        entry: Dict[str, Any] = {
            "name": name,
            "total_ms": round(us / 1000.0, 3),
            "count": count[name],
        }
        if sm_hits[name] > 0:
            # UI expects a 0..1 fraction; torch's "occupancy %" is already
            # 0..100, so scale.
            entry["sm_util"] = round(sm_sum[name] / sm_hits[name] / 100.0, 4)
        else:
            entry["sm_util"] = None
        out.append(entry)
    out.sort(key=lambda k: k["total_ms"], reverse=True)
    return out
