# AIProf 项目介绍

AIProf 是一套面向 AI / GPU 工作负载的开源性能分析平台。它把「无侵入式采集 → 报告生成 →
LLM 归因」三段合成一条端到端流水线：把生产环境里的一次 GPU/PyTorch 任务，5 秒内变成
一张融合 Python 栈 + CUDA kernel 的火焰图，再让本地或云端 LLM 给出
可读的瓶颈结论。

- 定位：AI 训练/推理任务的**性能剖析 + 智能诊断**
- 交付形态：两镜像客户端-服务端架构，`docker compose up` 即可跑通
- 目标读者：训练/推理基础设施工程师、模型工程师、SRE

---

## 1. 核心能力

### 1.1 无侵入采集（agent 侧）

一次 `POST /api/v1/app_observ/aiAnalysis/start_ai_analysis` 就能对指定 PID 触发采集，
不需要重启目标进程、不需要改一行业务代码。当前落地的采集器：

| 采集器 | 目标数据 | 依赖 | 状态 |
|---|---|---|---|
| **Pyki** | Python 调用栈（用户态注入 CPython，纳秒级取栈） | ptrace | ✅ 通用可用 |
| **CUPTI** | CUDA kernel 事件、显存拷贝、时间线 | cuprof 插件 + NVIDIA 驱动 | ✅ 通用可用 |
| **PyTorch tracer** | Python 栈 ↔ CUDA kernel 融合时间线 | Pyki + CUPTI | ✅ 通用可用 |
| **memory monitor** | 目标进程 RSS 峰值 | procfs | ✅ 通用可用 |

> 如需 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标采集，请到阿里云操作系统控制台使用。

采集结束后把中间产物打包成 `.tar.gz`，通过 dashrs 的 multipart HTTP 上传回 server，
两台机器**不需要**共享卷。

### 1.2 报告生成（server 侧）

- Rust CollectionFramework 输出的原始数据在 server 侧由 `analysis_summary`
  （PyInstaller 打的 onefile）做二次聚合，出 `summary.json`。
- dashboardServer.js 把 summary + 火焰图 + 时间线拼成报告页，供前端 React
  webapp 渲染（Perfetto iframe 打开 kernel 时间线）。
- 结果全部落在 `<RESULT_DIR>/<analysisId>/`，重启不丢；每份报告都有独立 UUID
  可分享。

### 1.3 AI 分析结论（本地两条通路 + 云端跳转）

报告页顶部有「AI 分析结论」卡片，同一份 profiling 数据可以随时切换后端：

| 通路 | mode | 触发方式 | 典型场景 |
|---|---|---|---|
| **本地 Agent** | `local` | server 侧 `local-profiling-agent`：解析 chrome-tracing → 固定指标工具 → LLM 多轮 refine 综合结论 | 快速本地分析、无外部依赖 |
| **本地 OpenClaw** | `openclaw` | server 侧 spawn `openclaw agent --local` 走 agentic 循环 + MCP 工具链 | 想要多轮工具调用、深度归因 |
| **阿里云 Agent** | — | 「阿里云 Agent 分析 ↗」按钮跳转外部 Demo 环境（`VITE_ALIYUN_AGENT_URL`），上传 JSON 后由云端 Profiling Agent 分析 | 需要专业 GPU 诊断的云上体验 |

本地两条通路只需配置任一 OpenAI 兼容 LLM 的 Base URL + API Key（设置页或环境变量）。
云端深度分析不在开源范围内，通过外链跳转体验。

结论以 markdown 渲染（`react-markdown` + `remark-gfm`），支持代码块、表格、层级标题。

**持久化**：LLM 分析结论按 `analysisId` 落盘到 `<RESULT_DIR>/<analysisId>/ai_conclusion.json`，
跨浏览器都能看到、重启 server 也不丢。前端在「本地 OpenClaw 分析」按钮上做了确认：
若已有缓存结论，重新分析前会先弹窗提示、避免误覆盖。

### 1.4 dashboard / 前端

- Vite React SPA + Node/Express BFF（`dashboardServer.js`）
- 报告页三大区块：
  1. **元数据卡片** — analysisId、目标 PID / 实例名、采集时长、Peak RSS
  2. **AI 分析结论** — 三种 LLM 通路 + 收起/展开 + 缓存时间戳 + 清除按钮
  3. **原始数据视图** — 火焰图、Perfetto 时间线、内存快照（MemoryViz）、日志

---

## 2. 系统架构

```
┌────────────────────────────────┐        HTTP/WS         ┌──────────────────────────────┐
│           aiprof-server         │◀───────────────────────▶│           aiprof-client       │
│  ─────────────────────────────  │                        │  ──────────────────────────── │
│  • dashboardServer.js (17000)   │  ws://…/ws (任务分发) │  • WS agent (拉任务)          │
│  • webapp dist (Vite React)     │  HTTP multipart (上传) │  • CollectionFramework (Rust)│
│  • analysis_summary (PyInstall) │                        │  • Pyki / CUPTI plugins      │
│  • AI 通路: local/openclaw      │                        │  • dashrs (打包+上传)         │
│                                  │                        │                              │
│  • 结果 → /var/lib/aiprof/…     │                        │  → --pid=host --gpus all      │
└────────────────────────────────┘                        └──────────────────────────────┘
        │                                                                │
        └── 浏览器 ──── http://<server>:17000/aiprof ────────────────────┘
```

两个组件、一根 WebSocket、一条 HTTP 上传通道，就是全部对外依赖。

---

## 3. 容器镜像

AIProf 以两个镜像交付：

| 镜像 | 作用 | 端口 |
|---|---|---|
| `aiprof/server` | dashboard + AI 分析通路 + 报告页 + webapp | `17000` |
| `aiprof/client` | Rust CollectionFramework + WS agent（连 server 拉任务） | — |

- **aiprof/server** 基于 `node:22-bookworm-slim`（openclaw 硬要求 Node ≥ 22.22.3）
  - 多阶段构建：webapp (Vite) → analysis_summary (PyInstaller onefile) → runtime
  - `--build-arg WITH_OPENCLAW=1` 可在构建期预装 OpenClaw，镜像体积增加约 200MB
- **aiprof/client** 基于 `nvidia/cuda:12.2.0-runtime`
  - Rust workspace release build → 打包 Pyki / CUPTI vendored `.so`
  - 运行时要求 `--pid=host` + `CAP_SYS_PTRACE` + `--gpus all` + `nvidia-container-toolkit`

### 3.1 前置条件

**server 机器**（可无 GPU）
- Docker Engine ≥ 24
- 对 client 放行入方向端口 17000

**client 机器**（必须有 GPU）
- NVIDIA GPU + 驱动 + [nvidia-container-toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)
- 能 ptrace 目标进程 → 容器需 `--pid=host` + `CAP_SYS_PTRACE`

一键验 GPU 容器可用性：
```bash
docker run --rm --gpus all nvidia/cuda:12.2.0-runtime-ubuntu22.04 nvidia-smi
```

### 3.2 三种部署形态

#### A. 单机 compose（最快）

```bash
cd <repo root>
docker compose -f deploy/docker/docker-compose.yml up -d --build
```

首次 build：server ~5min，client ~15min。compose 里 client 走 `network_mode: host`，
用 `SERVER_HOST=127.0.0.1:17000` 直连 server。

一键自检（起栈 → 等注册 → 触发采集 → 拉报告）：
```bash
bash deploy/docker/smoke.sh
TARGET_PID=<pid> bash deploy/docker/smoke.sh    # 指定占 GPU 的 python 进程
```

#### B. 单机双容器脚本（参数可调）

```bash
bash deploy/docker/run-server.sh --build --port 17000
bash deploy/docker/run-client.sh --build --server 127.0.0.1:17000 --client-id local-gpu
```

#### C. 跨机部署

```
server 机器 (无需 GPU)                     client 机器 (带 GPU)
┌───────────────────────┐                 ┌───────────────────────────┐
│ aiprof-server         │◀── ws://…/ws ───│ aiprof-client              │
│  dashboard+WS+分析     │  + HTTP upload  │  CollectionFramework       │
│  :17000               │                 │  --pid host --gpus all     │
└───────────────────────┘                 └───────────────────────────┘
```

构建（context 必须是 repo 根，两个 Dockerfile 都要拷 `agent/` `server/` `ui/`）：
```bash
docker build -f deploy/docker/Dockerfile.server -t aiprof/server:latest .
docker build -f deploy/docker/Dockerfile.client -t aiprof/client:latest .

# 分发到目标机器
docker save aiprof/server:latest | ssh root@<server-ip> 'docker load'
docker save aiprof/client:latest | ssh root@<client-ip> 'docker load'
```

server 机器：
```bash
bash deploy/docker/run-server.sh --port 17000
```

client 机器：
```bash
bash deploy/docker/run-client.sh \
  --server <server-ip>:17000 \
  --client-id gpu-a10-01
```

### 3.3 常用运行时环境变量

**aiprof-server**
| 变量 | 默认 | 说明 |
|---|---|---|
| `PORT` | `17000` | 监听端口 |
| `RESULT_DIR` | `/var/lib/aiprof/result` | 结果 + AI 结论落盘目录 |
| `UPLOAD_TMP` | `/var/lib/aiprof/uploads` | client 上传中转 |
| `PERSISTENCE_DIR` | `/var/lib/aiprof/state` | OpenClaw 私有安装目录 |
| `QWEN_API_KEY` | — | 设了以后 OpenClaw 弹窗内 API Key 输入框可留空 |
| `AIPROF_OPENCLAW_MODEL` | `qwen/qwen3.5-plus` | OpenClaw 默认模型 |
| `AIPROF_OPENCLAW_TIMEOUT_S` | `300` | 单次 turn 超时秒 |
| `AIPROF_OPENCLAW_REGISTRY` | `https://registry.npmjs.org` | npmmirror 上 openclaw 是空壳，别改 |
| `ALIYUN_AGENT_URL` | `https://soma.openanolis.cn` | 「阿里云 Agent 分析 ↗」跳转目标（server 侧错误提示用；前端用 `VITE_ALIYUN_AGENT_URL`） |

**aiprof-client**
| 变量 | 默认 | 说明 |
|---|---|---|
| `SERVER_HOST` | `127.0.0.1:17000` | server 的 host:port（跨机必改） |
| `CLIENT_ID` | 主机名 | 注册到 server 的 clientId |
| `TRANSPORT` | `ws` | 控制平面传输；反代剥掉 WS `Upgrade` 头时设 `poll` 走 HTTP long-poll |
| `RUST_LOG` | `info` | 日志级别 |

---

## 4. 端到端验证

在 server 机器执行：

```bash
S=<server-ip>:17000   # 单机填 127.0.0.1:17000

# 1) client 注册成功？应看到 clientId、wsConnected=true
curl -fsS "http://$S/api/clients" | python3 -m json.tool

# 2) 触发一次 10s 采集（PID 换成 client 机器上占 GPU 的 python 进程）
curl -fsS -X POST "http://$S/api/v1/app_observ/aiAnalysis/start_ai_analysis" \
  -H 'Content-Type: application/json' \
  -d '{"instance":"gpu-a10-01","pids":"<PID>","timeout":10000}'
# 返回 { "analysisId": "<UUID>", ... }
```

- `instance` = clientId（`/api/clients` 里 `wsConnected=true` 那条）
- `timeout` 单位 ms
- 缺省同时启 gpu + pyki collector；target 上没跑 CUDA 会立刻返回
  `No process meets the conditions of the collector`

**打开报告**：
```
http://<server-ip>:17000/aiprof
http://<server-ip>:17000/api/v1/app_observ/aiAnalysis/report?analysisId=<UUID>
```

---

## 5. 排错速查

| 症状 | 原因 & 处置 |
|---|---|
| `/api/clients` 里看不到 client | WS `/ws` 没连上。`docker logs -f aiprof-client` 看日志；跨机多半是 `SERVER_HOST` 写错或安全组没放行 17000 |
| `/api/clients` 看不到 client，且 server 在反向代理后面 | 代理可能剥掉了 WS `Upgrade` 头，握手退化成普通 GET → `/ws` 404。给 client 设 `TRANSPORT=poll`（`run-client.sh --transport poll`）走 HTTP long-poll 备用通路 |
| client 起不来，报 `libnvidia-ml.so: cannot open shared object` | 非致命噪音，nvidia 采集器 init 失败但其他 collector 照跑；彻底消除得给容器补 `LD_LIBRARY_PATH` 或 vendor libnvidia-ml.so.1 |
| 采集触发后 `No processes meet the resource requirements` | 目标 PID 不存在或没跑 CUDA/torch。CF 通过 `nvidia-smi --query-compute-apps=pid` 校验；进程在别的容器里用 `--target <容器名>` |
| 上传失败 / 报告不生成 | 确认 server 端口对 client **出方向**可达；大 trace 走 HTTP，双向可达最稳 |
| Perfetto 页打不开 trace | 前端 iframe 走公网 `ui.perfetto.dev`，浏览器需能访问公网 |
| OpenClaw 分析报「未提供 API Key」 | 弹窗内填 Qwen API Key，或给 server 设 `QWEN_API_KEY` 后重启 |
| OpenClaw 结果显示成 JSON 而不是 markdown | 后端 `payloads` 取值走了 fallback 分支；检查 `dashboardServer.js` 里 `parsed?.payloads \|\| parsed?.result?.payloads` |

---

## 6. 相关文档

| 文件 | 说明 |
|---|---|
| `docs/deployment.md` | 完整 Docker 部署指南（compose / 参数化脚本 / 跨机） |
| `docs/architecture.md` | 组件划分、数据流、存储 schema |
| `docs/quickstart.md` | 五步跑通 demo |
| `docs/data-format.md` | 上传数据格式、meta.json/folded.txt/timeline.json 定义 |
| `deploy/docker/README.md` | 单机 compose 详解 + vendored `.so` 说明 |
| `deploy/docker/README-split.md` | 跨机部署速查 |

---

## 7. License

Apache-2.0。vendored 第三方源码（pyki）见 `agent/collection_framework/src/plugins/pyki/`。
