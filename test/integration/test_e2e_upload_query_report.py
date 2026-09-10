# SPDX-License-Identifier: Apache-2.0
"""End-to-end: agent upload -> collector -> query -> report, all via BFF.

Uses the reference Python agent in `--mock` mode so the test needs no
`perf` privileges and no GPU. Verifies the session is discoverable
through the same JSON APIs the browser calls.
"""

from __future__ import annotations

import json
import subprocess
import time
import urllib.request

import pytest


def _http_json(url: str, method: str = "GET", body: bytes | None = None) -> dict:
    req = urllib.request.Request(url, method=method, data=body,
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read())


def _http_text(url: str) -> str:
    with urllib.request.urlopen(url, timeout=30) as resp:
        return resp.read().decode("utf-8")


def test_bff_healthz(endpoints):
    got = _http_json(endpoints["bff"] + "/healthz")
    assert got.get("ok") is True
    assert got.get("service") == "bff"


def test_agent_upload_then_session_visible_via_bff(endpoints, repo_root, tmp_path, python_exe):
    workload_tag = f"pytest-mock-{int(time.time())}"
    records = tmp_path / "records"

    cmd = [
        python_exe, str(repo_root / "agent-py" / "aiprof_agent.py"),
        "--mock", "--once",
        "--duration", "2",
        "--workload", workload_tag,
        "--records-dir", str(records),
        "--endpoint", endpoints["collector"] + "/api/v1/collector/upload",
    ]
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
    assert out.returncode == 0, f"agent failed: {out.stderr}"

    # Grab the newest session with our tag from the query API (via BFF).
    listing = _http_json(endpoints["bff"] + "/api/v1/aiprof/sessions")
    ours = [s for s in listing["items"] if s.get("workload") == workload_tag]
    assert ours, f"no session with workload={workload_tag} in {listing}"
    sid = ours[0]["session_id"]

    # The folded flamegraph must be a valid Brendan-Gregg stream.
    folded = _http_text(endpoints["bff"] + f"/api/v1/aiprof/sessions/{sid}/folded")
    assert folded.strip(), "empty folded flamegraph"
    for line in folded.splitlines():
        stack, count = line.rsplit(" ", 1)
        assert stack
        assert count.isdigit()


def test_report_endpoint_returns_analysis(endpoints, repo_root, tmp_path, python_exe):
    workload_tag = f"pytest-report-{int(time.time())}"
    records = tmp_path / "records"

    cmd = [
        python_exe, str(repo_root / "agent-py" / "aiprof_agent.py"),
        "--mock", "--once",
        "--duration", "2",
        "--workload", workload_tag,
        "--records-dir", str(records),
        "--endpoint", endpoints["collector"] + "/api/v1/collector/upload",
    ]
    subprocess.run(cmd, capture_output=True, text=True, timeout=60, check=True)

    listing = _http_json(endpoints["bff"] + "/api/v1/aiprof/sessions")
    sid = next(s["session_id"] for s in listing["items"]
               if s.get("workload") == workload_tag)

    rep = _http_json(endpoints["bff"] + f"/api/v1/aiprof/report/{sid}",
                     method="POST", body=b"{}")
    assert rep["session_id"] == sid
    assert isinstance(rep.get("content"), str) and rep["content"].strip(), rep
