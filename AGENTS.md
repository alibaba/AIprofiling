# Repository Guidelines

AIProf profiles GPU/AI workloads end to end: a Rust agent captures Python
stacks + CUDA kernels, the server builds reports, and an LLM layer explains the
bottleneck. It ships as two images (`aiprof/server`, `aiprof/client`).

## Project Structure

- `agent/collection_framework/` — Rust workspace and collector plugins
  (`src/plugins/pyki`, `cuprof`, `pystackCollector`, `pykiLoader`) plus
  `src/third_party/`. Vendored trees keep their own `LICENSE`/`NOTICE` where
  upstream shipped one (`pyki/`, `cuprof/`); `cuprof/VENDOR.md` records its base
  commit and local patches. Anything new you vendor must arrive with its license
  text and a root `NOTICE` entry.
- `server/dashboard/` — Express BFF (`dashboardServer.js`; `PORT` defaults to
  7000, but every documented deployment maps it to 17000) plus the Python
  post-processing that `build.sh` bundles into `analysis_summary`.
- `server/services/`, `server/common/` — FastAPI services (collector 7101,
  query 7102, report 7103) and shared config/storage. **Not wired into
  `deploy/docker/docker-compose.yml`** — only `aiprof-server` and `aiprof-client`
  ship; treat these as standalone/legacy until you need them.
- `agent-ai/` — OpenAI-compatible LLM gateway (port 8642) and MCP tool schemas;
  likewise not part of the compose stack.
- `local-profiling-agent/` — Node CLI: trace ingest, evidence tools, LLM loop.
- `ui/webapp/` — Vite + React + TypeScript + Ant Design (the shipped UI;
  `src/pages/AIProf/` is the main report UI; `src/pages/ai_observable/` is NOT
  legacy — it hosts the CUDA memory-snapshot (MemoryViz) view that
  `ProfileTab.tsx` embeds as an iframe (`/ai_observable/result?embed=memviz`),
  and `dashboardServer.js` serves it via a static route). `ui/frontend/` and
  `ui/bff/` are an older static+proxy pair, undeployed.
- `deploy/docker/` — `Dockerfile.server`, `Dockerfile.client`, `docker-compose.yml`,
  `run-server.sh`, `run-client.sh`, `smoke.sh`, `README.md`, `README-split.md`.
  A second, divergent `Dockerfile.server.prebuilt` sits at the repo root.
- `test/` — pytest suites, `native/` (GPU-free C++ test for the vendored cuprof
  patches), CPU-only workloads, fixtures; `docs/` holds guides.
- `data/` — server runtime state (results, SQLite); gitignored.

## Build, Test, and Development Commands

```bash
cd agent/collection_framework && cargo build --release    # Rust collector
cd ui/webapp && npm ci && npm run build                   # tsc -b + vite build
cd server/dashboard && npm ci && node dashboardServer.js  # local BFF
cd server/dashboard && node --test *.test.js              # needs npm ci first
cd local-profiling-agent && npm ci && npm test            # node --test test/
python3.11 -m pytest test/ -v                             # test/unit needs no GPU
make test-native                                          # C++11 only, no GPU
cd deploy/docker && docker compose up -d --build && bash smoke.sh
make ui-build                                             # or make deploy-ui
```

## Coding Style & Naming

- New source files should start with `// SPDX-License-Identifier: Apache-2.0`
  (`#` in Python). This is **aspirational, not enforced**: none of the ~76
  tracked `.rs` files carry it and only ~69 of 228 first-party sources do. Add
  it to files you create; do not mass-add it to vendored trees.
- Rust: `cargo fmt` defaults; Clippy warnings are errors. The pre-commit config
  under `agent/collection_framework/` also has a `no-chinese-characters` hook,
  but it is **not enforced repo-wide today** — ~100 tracked files (including
  `src/main.rs`, `cupti_plugin_wrapper.rs`, `config.yaml`) already carry Chinese
  comments. Write new source comments in English; do not add new CJK to code.
- TypeScript/React: 2-space indent, single quotes, semicolons, `@/*` → `src/*`.
- Server/agent JS: CommonJS `require`, 4-space indent, `'use strict'`.
- Python 3.11: 4 spaces, double quotes, `@dataclass` settings. Configure via
  env vars; never hardcode paths or credentials.

## Testing Guidelines

- `test/native/` is a plain C++11 program with its own `make`; run it via
  `make test-native`. It is the only automated coverage of the vendored cuprof
  patch series, and `cuprof/VENDOR.md` lists it as a post-re-sync step.
- Node uses `node:test` + `node:assert`, with tests beside the code as
  `<module>.test.js`. Python tests are `test/unit/test_<area>.py` and
  `test/integration/test_*.py`; integration `pytest.skip`s without a live stack.
- Run `cargo test -- --test-threads=1`; plugins mutate process/GPU state.
- No coverage gate, but every behaviour change needs a test and all suites must
  pass before a PR.

## Commit & Pull Request Guidelines

- Conventional Commits, imperative, lowercase, no trailing period:
  `feat(webapp): support poll clients in the capture instance picker`. Types:
  `feat`, `fix`, `refactor`, `chore`, `docs`, `build`, `test`. Scopes:
  `webapp`, `ui`, `server`, `dashboard`, `cf`, `ai`, `client`, `deploy`. Explain
  the *why* in the body; one logical change per commit.
- PRs target `master`: motivation, summary, verification (commands + output),
  screenshots for UI work, and explicit notes on new env vars, ports, or compose
  changes. Never commit `.env*`, `data/`, `dist/`, `target/`, `docs/superpowers/`.

## Agent-Specific Instructions

- Prefer the narrowest loop (one test file, `cargo test -p <crate>`) before full
  suites or Docker builds.
- `dashboardServer.test.js` reports "server exited early" when `node_modules` is
  missing — run `npm ci` in `server/dashboard` first.
- Lockfile churn belongs in its own labelled commit; keep the configurable
  `BASE_PATH` behaviour intact when touching routes or static assets.
