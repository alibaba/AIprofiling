# SPDX-License-Identifier: Apache-2.0
"""End-to-end unit tests for the query service's GPU endpoints.

We spin up a FastAPI TestClient against a scratch DB + blob directory,
seed a session whose tarball contains a chrome-trace, then exercise:

  * /api/v1/aiprof/sessions/{sid}                — artifact manifest
  * /api/v1/aiprof/sessions/{sid}/timeline       — raw JSON pass-through
  * /api/v1/aiprof/sessions/{sid}/kernels        — aggregated stats
"""

from __future__ import annotations

import importlib
import io
import json
import os
import sys
import tarfile
import time
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))


def _pack_session(blob_dir: Path, sid: str, members: dict) -> str:
    key = f"{sid}.tar.gz"
    with tarfile.open(blob_dir / key, "w:gz") as tar:
        for name, data in members.items():
            info = tarfile.TarInfo(name=f"{sid}/{name}")
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return key


def _make_trace() -> bytes:
    return json.dumps(
        {
            "traceEvents": [
                {
                    "ph": "X", "cat": "kernel", "name": "sm90_gemm",
                    "dur": 500, "pid": 1, "tid": 2,
                    "args": {"est. achieved occupancy %": 60.0},
                },
                {
                    "ph": "X", "cat": "kernel", "name": "sm90_gemm",
                    "dur": 700, "pid": 1, "tid": 2,
                    "args": {"est. achieved occupancy %": 80.0},
                },
                {"ph": "X", "cat": "gpu_memcpy", "name": "MemcpyHtoD",
                 "dur": 100, "pid": 1, "tid": 2},
                {"ph": "X", "cat": "cpu_op", "name": "aten::matmul",
                 "dur": 999, "pid": 1, "tid": 2},
                {"ph": "M", "cat": "kernel", "name": "meta"},
            ]
        }
    ).encode()


@pytest.fixture()
def client(tmp_path, monkeypatch):
    # Point the service at scratch storage before importing.
    db = tmp_path / "aiprof.db"
    blobs = tmp_path / "blobs"
    blobs.mkdir()
    monkeypatch.setenv("AIPROF_DB_URL", f"sqlite:///{db}")
    monkeypatch.setenv("AIPROF_BLOB_DIR", str(blobs))

    # Force a fresh import so the module picks up the env.
    for name in list(sys.modules):
        if name.startswith("server."):
            del sys.modules[name]
    from server.common.store import Store  # noqa: E402
    main = importlib.import_module("server.services.query.app.main")

    # Seed a session with a torch-trace inside.
    sid = "s-test-1"
    key = _pack_session(blobs, sid, {"torch-trace.json": _make_trace(),
                                     "folded.txt": b"python;torch;CUDA:sm90_gemm 1200\n"})
    store = Store(f"sqlite:///{db}")
    store.insert_session(
        {
            "session_id": sid,
            "schema_version": 1,
            "host": "h",
            "workload": "w",
            "kind": "py-cuda",
            "pid": 1,
            "start_ts": int(time.time()) - 5,
            "end_ts": int(time.time()),
            "duration_s": 5,
            "languages": ["python", "native", "cuda"],
            "agent_ver": "test",
            "notes": "",
        },
        key,
    )
    yield TestClient(main.app), sid


def test_session_detail_lists_artifacts(client):
    c, sid = client
    r = c.get(f"/api/v1/aiprof/sessions/{sid}")
    assert r.status_code == 200
    body = r.json()
    assert body["session_id"] == sid
    assert body["artifacts"]["torch-trace.json"] is True
    assert body["artifacts"]["folded.txt"] is True
    assert body["artifacts"]["timeline.json"] is False


def test_timeline_endpoint_returns_raw_json(client):
    c, sid = client
    r = c.get(f"/api/v1/aiprof/sessions/{sid}/timeline")
    assert r.status_code == 200
    assert r.headers["content-type"].startswith("application/json")
    trace = r.json()
    assert "traceEvents" in trace
    assert any(e.get("cat") == "kernel" for e in trace["traceEvents"])


def test_kernels_endpoint_aggregates(client):
    c, sid = client
    r = c.get(f"/api/v1/aiprof/sessions/{sid}/kernels")
    assert r.status_code == 200
    items = r.json()["items"]
    by_name = {k["name"]: k for k in items}
    gemm = by_name["sm90_gemm"]
    assert gemm["count"] == 2
    # 500us + 700us = 1200us = 1.2ms
    assert gemm["total_ms"] == pytest.approx(1.2)
    # (60 + 80) / 2 / 100 = 0.7
    assert gemm["sm_util"] == pytest.approx(0.7)
    memcpy = by_name["MemcpyHtoD"]
    assert memcpy["count"] == 1
    assert memcpy["sm_util"] is None
    # cpu_op events must not leak into the GPU kernel table.
    assert "aten::matmul" not in by_name


def test_timeline_404_when_absent(client, tmp_path):
    c, _sid = client
    # Seed a CPU-only session and query its timeline.
    from server.common.store import Store
    from server.common.config import settings
    blobs_dir = Path(settings.blob_dir)
    sid2 = "s-cpu-only"
    key = _pack_session(blobs_dir, sid2, {"folded.txt": b"main;work 3\n"})
    store = Store(settings.db_url)
    store.insert_session(
        {
            "session_id": sid2,
            "schema_version": 1,
            "host": "h",
            "workload": "w",
            "kind": "cpu-oncpu",
            "pid": None,
            "start_ts": int(time.time()) - 1,
            "end_ts": int(time.time()),
            "duration_s": 1,
            "languages": ["native"],
            "agent_ver": "test",
            "notes": "",
        },
        key,
    )
    r = c.get(f"/api/v1/aiprof/sessions/{sid2}/timeline")
    assert r.status_code == 404
    r2 = c.get(f"/api/v1/aiprof/sessions/{sid2}/kernels")
    assert r2.status_code == 200
    assert r2.json()["items"] == []
