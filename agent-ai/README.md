# AIProf AI analysis gateway

A tiny OpenAI-compatible endpoint that fronts an LLM for AIProf's
"analyze session" feature. Two operating modes:

1. **Upstream mode** — forwards to any OpenAI-compatible API.

   ```
   AIPROF_LLM_UPSTREAM=https://api.openai.com/v1/chat/completions
   AIPROF_LLM_KEY=sk-...
   AIPROF_LLM_MODEL=gpt-4o-mini   # optional; default gpt-4o-mini
   ```

2. **Offline heuristic mode** — no external calls; parses the folded stacks
   embedded in the prompt and returns a deterministic summary. Ideal for
   CI, air-gapped demos, or contributors without an API key.

Local auth token: `AIPROF_AI_KEY` (defaults to `aiprof-local-key`).

## Run

```bash
pip install -r requirements.txt
uvicorn agent-ai.gateway.main:app --port 8642
```

## API

- `GET  /v1/models`
- `POST /v1/chat/completions` — request/response schema matches OpenAI's
  `chat.completions` (subset).

Both endpoints require `Authorization: Bearer $AIPROF_AI_KEY`.

## MCP-style tool surface

The AI gateway can act as a client for three MCP-style tools exposed by
the AIProf query service. See `mcp/tools/` for JSON schemas. Wire your
preferred MCP framework to those schemas; the reference gateway only
demonstrates the OpenAI-compatible surface used by `server/report`.
