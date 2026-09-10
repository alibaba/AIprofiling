# SPDX-License-Identifier: Apache-2.0
"""AIProf report service — bridges UI to the AI analysis gateway.

Given a session id, fetches its folded stacks from the query service,
crafts a prompt, and asks `agent-ai` (an OpenAI-compatible endpoint) for
an analysis. Result is cached in the store so a second click returns
immediately.
"""

from __future__ import annotations

import json
from typing import Optional

import httpx
from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware

from server.common.config import settings
from server.common.storage import BlobStore
from server.common.store import Store


app = FastAPI(title="aiprof-report", version="0.1.0")
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
    return {"ok": True, "service": "report"}


@app.post("/api/v1/aiprof/report/{session_id}")
async def generate_report(session_id: str, refresh: bool = False):
    if not refresh:
        cached = store.get_report(session_id, mode="ai")
        if cached:
            return {"session_id": session_id, "cached": True, "content": cached["content"]}

    s = store.get_session(session_id)
    if not s:
        raise HTTPException(404, "session not found")
    folded_bytes = blobs.extract_member(s["blob_key"], "folded.txt")
    folded = (folded_bytes or b"").decode("utf-8", errors="replace")
    # Bound the prompt: keep the top N heaviest stacks.
    top = _top_stacks(folded, n=30)

    prompt = (
        "You are AIProf's performance analyst. Analyze the following top "
        "sampled stacks from a "
        f"{s['kind']} session on host {s['host']}. Point out the dominant "
        "hotspot(s), any suspicious wait patterns, and one concrete "
        "optimization suggestion. Keep the reply under 200 words.\n\n"
        "```\n" + top + "\n```"
    )

    content = await _ask_ai(prompt)
    store.upsert_report(session_id, mode="ai", content=content)
    return {"session_id": session_id, "cached": False, "content": content}


@app.get("/api/v1/aiprof/report/{session_id}")
def get_cached_report(session_id: str):
    r = store.get_report(session_id, mode="ai")
    if not r:
        raise HTTPException(404, "no report; POST first to generate")
    return {"session_id": session_id, "content": r["content"]}


def _top_stacks(folded: str, n: int) -> str:
    lines = []
    for line in folded.splitlines():
        line = line.rstrip()
        if not line:
            continue
        try:
            stack, count = line.rsplit(" ", 1)
            lines.append((int(count), stack))
        except (ValueError, TypeError):
            continue
    lines.sort(reverse=True)
    return "\n".join(f"{s} {c}" for c, s in lines[:n])


async def _ask_ai(prompt: str) -> str:
    if not settings.ai_api_key:
        # Deterministic offline fallback so the UI stays usable in demos.
        return (
            "[AI report unavailable — AIPROF_AI_KEY is not set. "
            "Set OPENAI_API_KEY or AIPROF_AI_KEY on the report service to "
            "enable live analysis.]"
        )
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {settings.ai_api_key}",
    }
    body = {
        "model": "aiprof-agent",
        "messages": [
            {"role": "system", "content": "You are a concise performance analyst."},
            {"role": "user", "content": prompt},
        ],
        "stream": False,
    }
    async with httpx.AsyncClient(timeout=60.0) as cx:
        r = await cx.post(settings.ai_gateway_url, headers=headers, json=body)
        r.raise_for_status()
        data = r.json()
    try:
        return data["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        return json.dumps(data)[:2000]
