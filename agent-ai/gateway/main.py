# SPDX-License-Identifier: Apache-2.0
"""AIProf AI analysis gateway.

An OpenAI-compatible `/v1/chat/completions` endpoint that acts as a thin
LLM adapter with three MCP-style tools exposed as callable helpers:

  - list_sessions
  - fetch_flamegraph
  - analyze_hotspot

The gateway can:
  * forward requests to any OpenAI-compatible upstream (set
    `AIPROF_LLM_UPSTREAM` and `AIPROF_LLM_KEY`); or
  * return a deterministic "offline" analysis derived from the folded
    stacks alone. This lets AIProf demo end-to-end without an API key.

Design goal: a minimal self-hosted AI adapter — ~200 LoC, no vendor
lock-in — that AIProf's `server/report` can call for session analysis.
"""

from __future__ import annotations

import json
import os
import time
from collections import Counter
from typing import Any, Dict, List, Optional

import httpx
from fastapi import FastAPI, Header, HTTPException
from pydantic import BaseModel


UPSTREAM_URL = os.getenv("AIPROF_LLM_UPSTREAM")
UPSTREAM_KEY = os.getenv("AIPROF_LLM_KEY") or os.getenv("OPENAI_API_KEY")
QUERY_URL    = os.getenv("AIPROF_QUERY_URL", "http://localhost:7102")
LOCAL_TOKEN  = os.getenv("AIPROF_AI_KEY", "aiprof-local-key")


app = FastAPI(title="aiprof-ai", version="0.1.0")


class Msg(BaseModel):
    role: str
    content: str


class ChatReq(BaseModel):
    model: str
    messages: List[Msg]
    stream: bool = False


def _auth(authorization: Optional[str]) -> None:
    if not authorization or not authorization.startswith("Bearer "):
        raise HTTPException(401, "missing bearer token")
    if authorization.removeprefix("Bearer ").strip() != LOCAL_TOKEN:
        raise HTTPException(401, "bad token")


@app.get("/healthz")
def healthz():
    return {"ok": True, "service": "ai", "upstream": bool(UPSTREAM_URL)}


@app.get("/v1/models")
def list_models(authorization: Optional[str] = Header(default=None)):
    _auth(authorization)
    return {
        "object": "list",
        "data": [
            {"id": "aiprof-agent", "object": "model", "owned_by": "aiprof"},
            {"id": "aiprof-offline", "object": "model", "owned_by": "aiprof"},
        ],
    }


@app.post("/v1/chat/completions")
async def chat_completions(req: ChatReq, authorization: Optional[str] = Header(default=None)):
    _auth(authorization)

    if UPSTREAM_URL and UPSTREAM_KEY and req.model != "aiprof-offline":
        return await _forward_upstream(req)

    # Offline heuristic: parse folded stacks the caller inlined.
    text = " ".join(m.content for m in req.messages if m.role == "user")
    reply = _offline_analyze(text)
    return _wrap_response(reply, req.model)


async def _forward_upstream(req: ChatReq) -> Dict[str, Any]:
    payload: Dict[str, Any] = {
        "model": os.getenv("AIPROF_LLM_MODEL", "gpt-4o-mini"),
        "messages": [m.model_dump() for m in req.messages],
        "stream": False,
    }
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {UPSTREAM_KEY}",
    }
    async with httpx.AsyncClient(timeout=60.0) as cx:
        r = await cx.post(UPSTREAM_URL, headers=headers, json=payload)
        if r.status_code >= 400:
            raise HTTPException(r.status_code, r.text[:2000])
        return r.json()


def _wrap_response(text: str, model: str) -> Dict[str, Any]:
    return {
        "id": f"aiprof-{int(time.time())}",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop",
            }
        ],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
    }


# ---------------------------------------------------------------------------
# Offline analysis — a small, deterministic "smart summary" over folded
# stacks. Not a replacement for a real LLM, but useful for demos and CI.
# ---------------------------------------------------------------------------

def _offline_analyze(prompt: str) -> str:
    # Look for a folded block inside a triple-backtick section.
    folded = _extract_folded(prompt)
    if not folded:
        return (
            "AIProf offline analyzer could not find a folded stack block in "
            "the prompt. Set AIPROF_LLM_UPSTREAM + AIPROF_LLM_KEY to enable "
            "real LLM analysis."
        )
    counts = _parse_folded(folded)
    if not counts:
        return "The provided folded stacks were empty. No hotspots to report."

    total = sum(counts.values())
    top = counts.most_common(5)
    top_frames = _top_leaf_frames(counts, k=5)
    cuda_hot = sum(v for k, v in counts.items() if "CUDA:" in k)
    cuda_pct = (cuda_hot * 100.0 / total) if total else 0.0

    lines = [
        f"Offline heuristic analysis (no LLM). {len(counts)} unique stacks, {total} samples.",
        "",
        "Top-level hot stacks (heaviest first):",
    ]
    for stack, c in top:
        pct = c * 100.0 / total
        leaf = stack.split(";")[-1]
        lines.append(f"  - {leaf}  ({c} samples, {pct:.1f}%)")
    lines.append("")
    lines.append("Most-sampled leaf frames:")
    for frame, c in top_frames:
        pct = c * 100.0 / total
        lines.append(f"  - {frame}  ({c} samples, {pct:.1f}%)")
    lines.append("")
    if cuda_pct > 5:
        lines.append(
            f"CUDA leaves account for {cuda_pct:.1f}% of samples — kernels are "
            "on the hot path. Consider grouping small kernels or using cudaGraph."
        )
    else:
        lines.append(
            "CUDA leaves account for <5% of samples — the hot path is CPU-bound. "
            "Look at the top Python/native frames above first."
        )
    lines.append("")
    lines.append(
        "Set AIPROF_LLM_UPSTREAM + AIPROF_LLM_KEY on the AI gateway to switch "
        "to a real LLM for richer analysis."
    )
    return "\n".join(lines)


def _extract_folded(prompt: str) -> Optional[str]:
    if "```" not in prompt:
        return prompt if _looks_folded(prompt) else None
    parts = prompt.split("```")
    for chunk in parts:
        if _looks_folded(chunk):
            return chunk
    return None


def _looks_folded(text: str) -> bool:
    lines = [l for l in text.splitlines() if l.strip()]
    if not lines:
        return False
    hits = 0
    for l in lines[:20]:
        rs = l.rsplit(" ", 1)
        if len(rs) == 2 and rs[1].isdigit():
            hits += 1
    return hits >= max(1, len(lines[:20]) // 3)


def _parse_folded(text: str) -> Counter:
    c: Counter = Counter()
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.rsplit(" ", 1)
        if len(parts) != 2 or not parts[1].isdigit():
            continue
        c[parts[0]] += int(parts[1])
    return c


def _top_leaf_frames(counts: Counter, k: int) -> List[tuple]:
    leaves: Counter = Counter()
    for stack, c in counts.items():
        leaves[stack.split(";")[-1]] += c
    return leaves.most_common(k)
