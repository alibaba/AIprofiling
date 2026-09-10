# SPDX-License-Identifier: Apache-2.0
"""AIProf collector service.

Accepts session tarballs from agents, stores blob + metadata.
"""

from __future__ import annotations

import json
from typing import Optional

from fastapi import FastAPI, File, Form, Header, HTTPException, UploadFile

from server.common.config import settings
from server.common.storage import BlobStore
from server.common.store import Store


app = FastAPI(title="aiprof-collector", version="0.1.0")

store = Store(settings.db_url)
blobs = BlobStore(settings.blob_dir)


@app.get("/healthz")
def healthz():
    return {"ok": True, "service": "collector"}


@app.post("/api/v1/collector/upload")
async def upload(
    meta: UploadFile = File(...),
    blob: UploadFile = File(...),
    authorization: Optional[str] = Header(default=None),
):
    _check_token(authorization)

    meta_bytes = await meta.read()
    try:
        meta_json = json.loads(meta_bytes.decode("utf-8"))
    except Exception as e:
        raise HTTPException(400, f"meta.json parse: {e}")

    required = ("session_id", "host", "kind", "start_ts", "end_ts")
    missing = [k for k in required if k not in meta_json]
    if missing:
        raise HTTPException(400, f"meta.json missing: {missing}")

    session_id = str(meta_json["session_id"])
    blob_bytes = await blob.read()

    # tarball filename may be anything; we key by session_id + original suffix.
    orig = blob.filename or "session.tar.gz"
    suffix = ".tar.gz" if orig.endswith(".tar.gz") else (".tar.zst" if orig.endswith(".tar.zst") else ".tar")
    blob_key = f"{session_id}{suffix}"
    blobs.put_bytes(blob_key, blob_bytes)

    store.insert_session(meta_json, blob_key)
    return {"ok": True, "session_id": session_id, "blob_key": blob_key}


def _check_token(authorization: Optional[str]) -> None:
    if not settings.upload_token:
        return
    if not authorization or not authorization.startswith("Bearer "):
        raise HTTPException(401, "missing bearer token")
    if authorization.removeprefix("Bearer ").strip() != settings.upload_token:
        raise HTTPException(401, "bad token")
