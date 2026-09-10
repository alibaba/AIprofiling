# AIProf

> English: [`README.md`](README.md)


**AIProf** 是一套面向 AI / GPU 工作负载的开源性能分析平台，把
「无侵入采集 → 报告生成 → LLM 归因」串成一条端到端流水线：一次生产环境
GPU/PyTorch 任务，几秒内就能产出融合 Python 栈 + CUDA kernel
的时间线，再由本地或云端 LLM 给出可读的瓶颈结论。

- **定位**：AI 训练 / 推理任务的性能剖析 + 智能诊断
- **交付形态**：两镜像 client / server 架构，`docker compose up` 起栈
- **目标读者**：训练 / 推理基础设施工程师、模型工程师、SRE

完整介绍见 [`docs/introduction.md`](docs/introduction.md)。

---

## 核心能力

- **无侵入采集**（Rust + 用户态注入）
  - Pyki（Python 栈）、CUPTI（CUDA kernel）、PyTorch tracer、memory monitor —— 通用可用
  - 如需 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标，请到阿里云操作系统控制台使用
- **报告生成**：Rust CollectionFramework 输出 → `analysis_summary`
  (PyInstaller onefile) 二次聚合 → 前端 Perfetto iframe + 火焰图 + MemoryViz
- **AI 分析结论**（同一份 profiling 三种通路，可切换）
  - **本地 Agent** (`mode=local`)：解析 chrome-tracing → 固定指标工具 → LLM
    多轮 refine 综合结论。无外部依赖，快速本地分析。
  - **本地 OpenClaw** (`mode=openclaw`)：spawn `openclaw agent --local` 走
    agentic 循环 + MCP 工具链，适合深度归因。
  - **阿里云 Agent**：按钮跳转外部 Demo 环境（`VITE_ALIYUN_AGENT_URL`
    默认 `https://soma.openanolis.cn`），上传 JSON 由云端分析。云端能力
    不在本仓库范围内，通过外链体验。

两条本地通路只需配置一个 OpenAI 兼容 LLM 的 Base URL + API Key。

---

## 仓库结构

| 路径 | 作用 |
|---|---|
| `agent/collection_framework/` | Rust workspace：采集框架 + 各类 collector 插件（Pyki / CUPTI） |
| `agent-ai/` | AI 分析后端与工具链（本地 Agent / OpenClaw 集成） |
| `local-profiling-agent/` | 本地 Agent：chrome-tracing 指标提取 + LLM refine 循环 |
| `server/dashboard/` | Node/Express BFF (`dashboardServer.js`) + PyInstaller 打的 `analysis_summary` |
| `ui/webapp/` | Vite + React 前端（Ant Design，Perfetto iframe，MemoryViz） |
| `deploy/docker/` | 双镜像 Dockerfile、`docker-compose.yml`、`run-server.sh` / `run-client.sh` / `smoke.sh` |
| `docs/` | 项目介绍、部署指南、架构说明、快速开始、数据格式 |
| `data/` | server 运行时数据目录（结果、SQLite、上传缓存） |

---

## 快速上手

单机（server + client 起在同一台带 GPU 的机器上）：

```bash
cd deploy/docker
docker compose up -d --build            # 首次 build：server ~5min，client ~15min
bash smoke.sh                           # 一键自检：起栈 → 等注册 → 采集 → 拉报告
# 或指定占 GPU 的 python 进程 PID
TARGET_PID=<pid> bash smoke.sh
```

打开 [http://localhost:17000/aiprof](http://localhost:17000/aiprof)。

跨机（server 无 GPU，client 有 GPU，两台通过 17000 端口互通）：

```bash
# server 机器
bash deploy/docker/run-server.sh --build --port 17000

# client 机器
bash deploy/docker/run-client.sh --build --server <server-ip>:17000 --client-id gpu-a10-01
```

完整部署指南见 [`docs/deployment.md`](docs/deployment.md)，架构与数据流见
[`docs/architecture.md`](docs/architecture.md)，五步跑通 demo 见
[`docs/quickstart.md`](docs/quickstart.md)。

---

## 前置条件

**server 机器**（可无 GPU）
- Docker Engine ≥ 24
- 对 client 放行入方向端口 17000

**client 机器**（必须有 GPU）
- NVIDIA GPU + 驱动 + [`nvidia-container-toolkit`](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)
- 容器需 `--pid=host` + `CAP_SYS_PTRACE` + `--gpus all`（Pyki 走 ptrace 注入目标进程；
  没有 `--pid=host` 时 client 心跳里的 GPU 进程列表也会为空，dashboard 上看不到目标进程）

一键验证 GPU 容器可用性：

```bash
docker run --rm --gpus all nvidia/cuda:12.2.0-runtime-ubuntu22.04 nvidia-smi
```

---

## 从源码构建

- **Rust 采集框架**：`cd agent/collection_framework && cargo build --release`
- **前端**：`cd ui/webapp && npm install && npm run build`
- **`analysis_summary`（PyInstaller onefile）**：`cd server/dashboard &&
  python3.11 -m PyInstaller analysis_summary.spec`
- **两个镜像**（context 是 repo 根）：
  ```bash
  docker build -f deploy/docker/Dockerfile.server -t aiprof/server:latest .
  docker build -f deploy/docker/Dockerfile.client -t aiprof/client:latest .
  ```

### CUDA 版本兼容性

client 镜像在 build 时把 CUPTI 的 `libcupti.so.<major>` soname 烧进
`libcuprof.so`。这个 major 必须与 client 宿主机 NVIDIA 驱动的 CUDA 版本
（见 `nvidia-smi` → "CUDA Version"）一致；不一致时 cuprof 注入目标进程
的瞬间会直接 SIGSEGV，且不会有可读日志。

默认使用 CUDA 12.2。要针对其他 toolkit 构建，传入 `CUDA_VERSION`
（必要时也可以覆盖 Ubuntu 基础镜像 tag）：

```bash
docker build -f deploy/docker/Dockerfile.client \
    --build-arg CUDA_VERSION=13.0.0 \
    -t aiprof/client:cuda13 .
```

vendored CUPTI 版本矩阵和 runtime 挑选逻辑见
[`agent/collection_framework/src/third_party/cupti/README.md`](agent/collection_framework/src/third_party/cupti/README.md)。

---

## License

Apache-2.0。见 `LICENSE`。vendored 第三方依赖见 `agent/collection_framework/src/plugins/pyki/`（含 pyki 的 `LICENSE`）与 `src/third_party/`。

两个组件以非 Apache 条款发布：

- **libprofiler**（预编译 `.a`，位于
  `agent/collection_framework/src/third_party/profiler/`）—— 按同目录
  `LIBPROFILER-EULA.txt` 授权；仅公开头文件和 linker script 保持 Apache-2.0。
- **NVIDIA CUPTI**（`agent/collection_framework/src/third_party/cupti/`
  下的 `libcupti.so.*`）—— 按 NVIDIA CUDA Toolkit EULA
  （<https://docs.nvidia.com/cuda/eula/>）再分发。
