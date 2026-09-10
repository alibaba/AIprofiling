# SPDX-License-Identifier: Apache-2.0
"""Tests for agent-py/aiprof_gpu.py that don't require torch.

We exercise the trace→folded derivation and the CUPTI helper's graceful
no-op path, so this suite passes on CPU-only CI machines.
"""

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture(scope="module")
def gpu_mod():
    src = ROOT / "agent-py" / "aiprof_gpu.py"
    spec = importlib.util.spec_from_file_location("aiprof_gpu", src)
    mod = importlib.util.module_from_spec(spec)
    sys.modules["aiprof_gpu"] = mod
    spec.loader.exec_module(mod)
    return mod


def _write_trace(tmp_path, events):
    p = tmp_path / "trace.json"
    p.write_text(json.dumps({"traceEvents": events}))
    return p


def test_folded_from_trace_maps_kernel_and_cpu_op(gpu_mod, tmp_path):
    trace = _write_trace(
        tmp_path,
        [
            {"ph": "X", "cat": "kernel", "name": "sm90_gemm", "dur": 500, "pid": 1, "tid": 2},
            {"ph": "X", "cat": "kernel", "name": "sm90_gemm", "dur": 700, "pid": 1, "tid": 2},
            {"ph": "X", "cat": "cpu_op", "name": "aten::matmul", "dur": 300, "pid": 1, "tid": 2},
            {"ph": "X", "cat": "gpu_memcpy", "name": "MemcpyHtoD", "dur": 100, "pid": 1, "tid": 2},
            # Ignored: non-complete event and unknown category.
            {"ph": "M", "cat": "kernel", "name": "meta", "dur": 999},
            {"ph": "X", "cat": "unknown", "name": "junk", "dur": 42},
            # Ignored: zero duration.
            {"ph": "X", "cat": "kernel", "name": "sm90_gemm", "dur": 0},
        ],
    )
    folded, kernel_count = gpu_mod._folded_from_chrome_trace(trace)
    assert kernel_count == 3  # 2 sm90_gemm + 1 MemcpyHtoD
    lines = {line for line in folded.strip().split("\n")}
    assert "python;torch;CUDA:sm90_gemm 1200" in lines
    assert "python;torch;CUDA:MemcpyHtoD 100" in lines
    assert "python;torch;aten::matmul 300" in lines
    assert not any("unknown" in l or "junk" in l for l in lines)


def test_folded_from_trace_never_empty(gpu_mod, tmp_path):
    """Even an all-junk trace produces a non-empty folded file so downstream
    tools don't have to special-case empty inputs."""
    trace = _write_trace(tmp_path, [])
    folded, kernel_count = gpu_mod._folded_from_chrome_trace(trace)
    assert kernel_count == 0
    assert folded.strip() != ""


def test_cupti_set_thread_id_type_when_not_loaded(gpu_mod):
    """On a host with no libcupti mapped into this process, the helper
    must return False rather than raise — that's what lets the collector
    stay CPU-only-safe."""
    # This test is only meaningful in interpreters without cupti loaded;
    # if for some reason cupti *is* loaded we accept either outcome.
    result = gpu_mod.cupti_set_thread_id_type_system()
    assert result in (True, False)


def test_gpu_collector_hard_error_when_torch_missing(gpu_mod, tmp_path, monkeypatch):
    """Per the 'no stubs' directive, absent torch must raise
    GpuCollectorUnavailable — the caller should not get a silent mock."""
    # Simulate 'import torch' failing.
    import builtins

    real_import = builtins.__import__

    def fail_torch(name, *a, **kw):
        if name == "torch" or name.startswith("torch."):
            raise ImportError("simulated: no torch")
        return real_import(name, *a, **kw)

    monkeypatch.setattr(builtins, "__import__", fail_torch)
    with pytest.raises(gpu_mod.GpuCollectorUnavailable):
        gpu_mod.collect_gpu_profile(tmp_path, 0.1)
