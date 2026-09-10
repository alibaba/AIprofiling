# AIProf

> 中文文档: [`README.zh-CN.md`](README.zh-CN.md) · Detailed guides under [`docs/`](docs/) (currently in Chinese; English versions are on the roadmap — contributions welcome)

**AIProf** is an open-source performance analysis platform for AI / GPU
workloads. It stitches together **non-intrusive collection → report
generation → LLM-based attribution** into a single end-to-end pipeline:
run one production GPU / PyTorch job and, within seconds, get a merged
timeline of Python stacks and CUDA kernels, then a readable bottleneck
summary produced by a local or cloud LLM.

- **Focus**: performance profiling and intelligent diagnosis for AI training / inference jobs
- **Shape**: two-image (`client` / `server`) architecture, brought up with `docker compose up`
- **Audience**: infra engineers, model engineers, SREs

See [`docs/introduction.md`](docs/introduction.md) for a full overview.

---

## Core capabilities

- **Non-intrusive collection** (Rust + user-space injection)
  - Pyki (Python stacks), CUPTI (CUDA kernels), PyTorch tracer, memory monitor — all available out of the box
  - For richer signals (NVTX / NCCL / RDMA / DCGM / ROCm), use the Aliyun OS console offering
- **Report generation**: Rust `CollectionFramework` output → `analysis_summary` (PyInstaller onefile) aggregation → frontend Perfetto iframe + flame graph + MemoryViz
- **AI analysis conclusions** — three interchangeable paths on the same profile
  - **Local agent** (`mode=local`): parses chrome-tracing → fixed metric tools → LLM multi-round refine → synthesized conclusion. No external dependency, quick local analysis.
  - **Local OpenClaw** (`mode=openclaw`): spawns `openclaw agent --local` with an agentic loop + MCP toolchain, suited to deeper attribution.
  - **Aliyun agent**: a button that opens an external demo environment (`VITE_ALIYUN_AGENT_URL`, default `https://soma.openanolis.cn`) and uploads the JSON for cloud analysis. The cloud capability lives outside this repository.

Both local paths only need an OpenAI-compatible LLM Base URL + API Key.

---

## Repository layout

| Path | Purpose |
|---|---|
| `agent/collection_framework/` | Rust workspace: collection framework + collector plugins (Pyki / CUPTI) |
| `agent-ai/` | AI-analysis backend and tool chain (local agent / OpenClaw integration) |
| `local-profiling-agent/` | Local agent: chrome-tracing metric extraction + LLM refine loop |
| `server/dashboard/` | Node/Express BFF (`dashboardServer.js`) + the PyInstaller-packed `analysis_summary` |
| `ui/webapp/` | Vite + React frontend (Ant Design, Perfetto iframe, MemoryViz) |
| `deploy/docker/` | Two Dockerfiles, `docker-compose.yml`, `run-server.sh` / `run-client.sh` / `smoke.sh` |
| `docs/` | Introduction, deployment guide, architecture, quickstart, data format |
| `data/` | Server runtime data (results, SQLite, upload cache) |

---

## Quickstart

Single host (server + client on the same GPU box):

```bash
cd deploy/docker
docker compose up -d --build            # first build: server ~5min, client ~15min
bash smoke.sh                           # bring up stack → wait for registration → collect → fetch report
# or point at a specific GPU-holding python PID
TARGET_PID=<pid> bash smoke.sh
```

Then open [http://localhost:17000/aiprof](http://localhost:17000/aiprof).

Cross-host (server without GPU, client with GPU, port 17000 reachable):

```bash
# on the server host
bash deploy/docker/run-server.sh --build --port 17000

# on the client host
bash deploy/docker/run-client.sh --build --server <server-ip>:17000 --client-id gpu-a10-01
```

Full deployment guide: [`docs/deployment.md`](docs/deployment.md).
Architecture and data flow: [`docs/architecture.md`](docs/architecture.md).
Five-step demo walkthrough: [`docs/quickstart.md`](docs/quickstart.md).
(These guides are currently in Chinese.)

---

## Prerequisites

**Server host** (GPU not required)
- Docker Engine ≥ 24
- Inbound port 17000 open to the client

**Client host** (GPU required)
- NVIDIA GPU + driver + [`nvidia-container-toolkit`](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)
- Container must run with `--pid=host` + `CAP_SYS_PTRACE` + `--gpus all`
  (Pyki uses ptrace to inject into target processes; without `--pid=host`,
  the client heartbeat also carries an empty `gpuProcs` list, so the
  dashboard cannot see any active GPU processes.)

Sanity check that a GPU container works:

```bash
docker run --rm --gpus all nvidia/cuda:12.2.0-runtime-ubuntu22.04 nvidia-smi
```

---

## Building from source

- **Rust collection framework**: `cd agent/collection_framework && cargo build --release`
- **Frontend**: `cd ui/webapp && npm install && npm run build`
- **`analysis_summary` (PyInstaller onefile)**: `cd server/dashboard && python3.11 -m PyInstaller analysis_summary.spec`
- **Two images** (build context is the repo root):
  ```bash
  docker build -f deploy/docker/Dockerfile.server -t aiprof/server:latest .
  docker build -f deploy/docker/Dockerfile.client -t aiprof/client:latest .
  ```

### CUDA version compatibility

The client image bakes CUPTI's `libcupti.so.<major>` soname into
`libcuprof.so` at build time. That major MUST match the client host's
NVIDIA driver (see `nvidia-smi` → "CUDA Version"). A mismatch triggers a
SIGSEGV inside the profiled target the moment cuprof is injected, with
no user-actionable log line.

The default is CUDA 12.2. To build against a different toolkit, pass
`CUDA_VERSION` (and, when needed, the base Ubuntu tag):

```bash
docker build -f deploy/docker/Dockerfile.client \
    --build-arg CUDA_VERSION=13.0.0 \
    -t aiprof/client:cuda13 .
```

See [`agent/collection_framework/src/third_party/cupti/README.md`](agent/collection_framework/src/third_party/cupti/README.md)
for the vendored CUPTI matrix and how the runtime picks a `.so`.

---

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Issues and pull requests are welcome — English and Chinese are both fine.

---

## License

Apache-2.0. See `LICENSE`. Vendored third-party dependencies are under
`agent/collection_framework/src/plugins/pyki/` (with pyki's own `LICENSE`)
and `src/third_party/`; see the top-level `NOTICE` for the summary.

Two components ship under non-Apache terms:

- **libprofiler** (prebuilt `.a` bundled under
  `agent/collection_framework/src/third_party/profiler/`) — licensed
  under the accompanying `LIBPROFILER-EULA.txt`; only the public header
  and linker script remain Apache-2.0.
- **NVIDIA CUPTI** (`libcupti.so.*` under
  `agent/collection_framework/src/third_party/cupti/`) — redistributed
  under the NVIDIA CUDA Toolkit EULA
  (<https://docs.nvidia.com/cuda/eula/>).
