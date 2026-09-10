# AIProf Architecture

## Components

### Node side (agent)

- **AIProf Agent** (Rust, `agent/framework`) — a plugin-driven collection
  framework. Each plugin implements a common `Plugin` trait and produces
  `Event`s that are written to a session-scoped directory on local disk.
  Built-in plugins:
  - `cpu-oncpu`   — on-CPU flamegraph via perf events (optionally wraps
    `profiler-core` for multi-language stack unwinding)
  - `cpu-offcpu`  — off-CPU flamegraph via `sched:sched_switch` tracepoint
  - `cupti`       — CUDA kernel timeline via the `cuprof` injected library
  - `py-cuda-tracer` — fused Python-stack + CUDA-kernel timeline
- **Uploader** — writes a `.tar.zst` per session and POSTs it to the collector.
- **Reference Python agent** (`agent-py/`) — a small, dependency-light agent
  that produces a valid AIProf session using `perf record`. Handy for CI and
  environments without the Rust toolchain.

### Server side (center)

Three FastAPI services, backed by SQLite (default) or MySQL and MinIO or the
local filesystem:

| Service | Port | Job |
|---|---|---|
| `collector` | 7101 | receives session tarballs, stores raw blobs, indexes meta |
| `query`     | 7102 | REST for the UI: list / detail / folded / diff |
| `report`    | 7103 | proxies to the AI analysis gateway, caches reports |

Storage schema (SQLite/MySQL):

```
sessions(id, session_id, host, workload, kind, start_ts, end_ts,
         languages, agent_ver, blob_key)
artifacts(id, session_id, kind, path, size, sha256)
reports(id, session_id, mode, content, created_ts)
```

Raw blobs (flamegraphs, folded stacks, CUPTI dumps) live in MinIO or on-disk
`data/blobs/`; SQL is only an index.

### UI

Single-page React (Vite) app served alongside a Node BFF that talks to the
three FastAPI services. Pages:

- `/aiprof/sessions` — list & filter
- `/aiprof/sessions/:id` — flamegraph + timeline + metadata
- `/aiprof/report/:id` — AI-generated report (SSE-streamed)

### AI Analysis Agent (`agent-ai/`)

A minimal OpenAI-compatible HTTP gateway (`/v1/chat/completions`) that
exposes three MCP-style tools:

- `list_sessions`
- `fetch_flamegraph`
- `analyze_hotspot`

Requires an `OPENAI_API_KEY`-style credential in the environment when
forwarding to an upstream model; otherwise runs a deterministic offline
heuristic so demos work with no key.

## Data flow

```
   PLUGIN(s)                         COLLECTOR                    QUERY / REPORT
       │                                 │                              │
       ▼                                 │                              │
 /var/lib/aiprof/records/<sid>/          │                              │
   meta.json                             │                              │
   folded.txt                            │                              │
   timeline.json                         │                              │
       │                                 │                              │
       ▼                                 │                              │
   session.tar.zst  ── POST ────────────▶│                              │
                                         ▼                              │
                                   sessions row  +  blob                │
                                                                        │
   UI  ── GET /api/v1/aiprof/sessions ───────────────────────────────▶  │
   UI  ── GET /api/v1/aiprof/sessions/{id}/folded ────────────────────▶ │
   UI  ── POST /api/v1/aiprof/report/{id}  ─────▶ report ─┐             │
                                                          ▼             │
                                                    agent-ai gateway    │
```

## Deployment shape

- **Single host / dev**: `deploy/docker/docker-compose.yml`.
- **Kubernetes**: `deploy/k8s/` — agent as DaemonSet, everything else as
  Deployments + ClusterIP Services.

## Extending

Add a new collector: implement `Plugin` in a new crate under
`agent/framework/crates/plugins/`, register it in `plugins/mod.rs`. A plugin
only has to produce a `session.tar.zst`-compatible directory; the framework
takes care of packaging and upload.
