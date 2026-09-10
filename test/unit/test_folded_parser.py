# SPDX-License-Identifier: Apache-2.0
"""Unit tests for the perf-script -> Brendan Gregg folded parser
(agent-py/aiprof_agent.py::fold_perf_script).
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "test" / "data"


def _load_agent_module():
    # agent-py's dashed directory name is not a valid Python package, so we
    # import the single file by path.
    src = ROOT / "agent-py" / "aiprof_agent.py"
    spec = importlib.util.spec_from_file_location("aiprof_agent", src)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["aiprof_agent"] = mod
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


agent = _load_agent_module()


def _folded_dict(folded_text: str) -> dict:
    out = {}
    for line in folded_text.splitlines():
        if not line.strip():
            continue
        stack, count = line.rsplit(" ", 1)
        out[stack] = int(count)
    return out


def test_fold_perf_script_counts_identical_stacks():
    raw = (DATA / "sample.perf-script").read_text()
    folded = agent.fold_perf_script(raw)
    d = _folded_dict(folded)

    # Two identical kernel samples collapse to one entry with count=2.
    kernel_key = "perf_pmu_enable;__intel_pmu_enable_all;native_write_msr"
    assert d.get(kernel_key) == 2

    # Two matmul_layer samples share the same call chain -> count=2.
    hot_key = "main;train_epoch;forward_pass;matmul_layer"
    assert d.get(hot_key) == 2


def test_fold_perf_script_output_is_valid_folded_format():
    raw = (DATA / "sample.perf-script").read_text()
    folded = agent.fold_perf_script(raw)
    for line in folded.splitlines():
        stack, count = line.rsplit(" ", 1)
        assert stack, "empty stack in folded output"
        assert count.isdigit(), f"non-numeric count in {line!r}"


def test_fold_perf_script_ignores_offsets_in_symbols():
    raw = (DATA / "sample.perf-script").read_text()
    folded = agent.fold_perf_script(raw)
    # +0x40 / +0x60 should have been stripped so both samples of
    # matmul_layer collapse together.
    assert "+0x" not in folded


def test_fold_perf_script_empty_input():
    assert agent.fold_perf_script("") == ""
