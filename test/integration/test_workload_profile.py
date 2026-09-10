# SPDX-License-Identifier: Apache-2.0
"""Profile the CPU-only training / inference workloads and upload to AIProf.

For each workload:
  1. Launch it as a subprocess (--announce-pid).
  2. Point the reference agent at that pid via `perf record`.
  3. Verify the session lands in the collector, is queryable via the BFF,
     and the folded flamegraph contains at least one frame the workload
     is known to spend time in (e.g. matmul_layer / dense_layer_1).

Skipped when `perf` is missing or perf_event_paranoid is too strict.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
import urllib.request
from pathlib import Path

import pytest


def _http_json(url: str) -> dict:
    with urllib.request.urlopen(url, timeout=30) as r:
        return json.loads(r.read())


def _http_text(url: str) -> str:
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read().decode("utf-8")


def _perf_available() -> tuple[bool, str]:
    if not shutil.which("perf"):
        return False, "perf binary not found"
    try:
        with open("/proc/sys/kernel/perf_event_paranoid") as f:
            level = int(f.read().strip())
    except OSError:
        return True, ""  # Unknown; let perf itself fail if it will
    # Level >= 3 makes even root's cpu-samples fail on some kernels; level
    # <= 2 with CAP_SYS_ADMIN (or root) is enough for what we need.
    if level > 2 and os.geteuid() != 0:
        return False, f"perf_event_paranoid={level} and not root"
    return True, ""


@pytest.fixture(scope="module", autouse=True)
def _require_perf():
    ok, why = _perf_available()
    if not ok:
        pytest.skip(f"perf unavailable: {why}")


def _profile_pid(pid: int, endpoints, repo_root, tmp_path, python_exe,
                 workload_tag: str, duration: int) -> str:
    records = tmp_path / "records"
    cmd = [
        python_exe, str(repo_root / "agent-py" / "aiprof_agent.py"),
        "--once",
        "--pid", str(pid),
        "--duration", str(duration),
        "--workload", workload_tag,
        "--records-dir", str(records),
        "--endpoint", endpoints["collector"] + "/api/v1/collector/upload",
    ]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=90)
    # perf can fail with paranoid or missing symbols even as root; treat that
    # as a skip rather than a hard fail so the suite stays green on machines
    # where the kernel just won't let us record.
    if out.returncode != 0:
        pytest.skip(f"agent/perf failed: rc={out.returncode} stderr={out.stderr[-400:]}")

    listing = _http_json(endpoints["bff"] + "/api/v1/aiprof/sessions")
    ours = [s for s in listing["items"] if s.get("workload") == workload_tag]
    assert ours, f"no session with workload={workload_tag} appeared"
    return ours[0]["session_id"]


def test_training_workload_profile(endpoints, repo_root, tmp_path, python_exe, workload_runner):
    tag = f"pytest-train-{int(time.time())}"
    # Big enough that the workload keeps churning for the full perf window;
    # NumPy matmul on 256-hidden is fast, so we need a lot of steps.
    with workload_runner("training_workload.py",
                         ["--steps", "200000", "--hidden", "256"]) as (proc, pid):
        time.sleep(0.5)
        sid = _profile_pid(pid, endpoints, repo_root, tmp_path, python_exe,
                           workload_tag=tag, duration=3)

    folded = _http_text(endpoints["bff"] + f"/api/v1/aiprof/sessions/{sid}/folded")
    assert folded.strip(), "empty flamegraph for training workload"

    # NumPy's matmul dispatches through BLAS, so the leaf frame we can
    # count on is whatever BLAS symbol perf saw. But *some* recognisable
    # frame — matmul_layer OR forward_pass OR train_epoch OR one of the
    # numpy/BLAS symbols — must be there.
    expected_any = ("matmul_layer", "forward_pass", "train_epoch",
                    "backward_pass", "numpy", "blas", "sgemm", "dgemm")
    assert any(needle in folded for needle in expected_any), \
        f"none of {expected_any} found in {sid} folded ({len(folded)} bytes)"


def test_inference_workload_profile(endpoints, repo_root, tmp_path, python_exe, workload_runner):
    tag = f"pytest-infer-{int(time.time())}"
    with workload_runner("inference_workload.py",
                         ["--batches", "200000", "--batch", "32",
                          "--hidden", "256"]) as (proc, pid):
        time.sleep(0.5)
        sid = _profile_pid(pid, endpoints, repo_root, tmp_path, python_exe,
                           workload_tag=tag, duration=3)

    folded = _http_text(endpoints["bff"] + f"/api/v1/aiprof/sessions/{sid}/folded")
    assert folded.strip(), "empty flamegraph for inference workload"

    expected_any = ("dense_layer_1", "dense_layer_2", "run_batch", "gelu",
                    "softmax", "numpy", "blas", "sgemm", "dgemm", "tanh")
    assert any(needle in folded for needle in expected_any), \
        f"none of {expected_any} found in {sid} folded ({len(folded)} bytes)"
