# SPDX-License-Identifier: Apache-2.0
"""Shared fixtures for AIProf integration tests.

These tests assume an AIProf stack is running locally on default ports.
If any service is unreachable we skip the whole module — no false
failures on machines that only want to run the unit tests.
"""

from __future__ import annotations

import os
import socket
import subprocess
import sys
import time
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]

DEFAULTS = {
    "collector": "http://127.0.0.1:7101",
    "query":     "http://127.0.0.1:7102",
    "report":    "http://127.0.0.1:7103",
    "bff":       "http://127.0.0.1:8080",
}


def _endpoints():
    return {
        "collector": os.environ.get("AIPROF_COLLECTOR_URL", DEFAULTS["collector"]),
        "query":     os.environ.get("AIPROF_QUERY_URL",     DEFAULTS["query"]),
        "report":    os.environ.get("AIPROF_REPORT_URL",    DEFAULTS["report"]),
        "bff":       os.environ.get("AIPROF_BFF_URL",       DEFAULTS["bff"]),
    }


def _reachable(url: str, timeout=1.0) -> bool:
    from urllib.parse import urlparse
    p = urlparse(url)
    try:
        with socket.create_connection((p.hostname, p.port), timeout=timeout):
            return True
    except OSError:
        return False


@pytest.fixture(scope="session")
def endpoints():
    eps = _endpoints()
    missing = [name for name, url in eps.items() if not _reachable(url)]
    if missing:
        pytest.skip(
            "AIProf services not reachable: " + ", ".join(missing) +
            " — start the stack and re-run, or run only test/unit."
        )
    return eps


@pytest.fixture(scope="session")
def repo_root():
    return ROOT


@pytest.fixture(scope="session")
def python_exe():
    # Prefer the interpreter that ran pytest; workloads only need numpy.
    return sys.executable


@pytest.fixture
def workload_runner(python_exe, repo_root):
    """Start a workload as a subprocess and yield its pid.

    Yields (proc, pid). Caller may profile the pid while it runs; on
    context exit we wait/kill so tests don't leak processes.
    """
    from contextlib import contextmanager

    @contextmanager
    def _run(script_rel: str, extra_args: list[str] | None = None):
        script = repo_root / "test" / "workloads" / script_rel
        cmd = [python_exe, str(script), "--announce-pid", *(extra_args or [])]
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            first = proc.stdout.readline().strip()
            assert first.startswith("PID="), f"workload did not announce pid: {first!r}"
            pid = int(first.split("=", 1)[1])
            yield proc, pid
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()

    return _run
