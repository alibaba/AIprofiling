# AIProf Quickstart

五步跑通端到端流水线：起栈 → 触发采集 → 拉报告 → 打开 UI → AI 归因。

完整介绍见 [`introduction.md`](introduction.md)，部署细节见
[`deployment.md`](deployment.md)。

---

## 0. 前置条件

**server 机器**（可无 GPU）
- Docker Engine ≥ 24（或 Podman + compose v2）
- 对 client 放行入方向端口 17000

**client 机器**（必须有 GPU）
- NVIDIA GPU + 驱动 + [`nvidia-container-toolkit`](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)
- 容器需 `--pid=host` + `CAP_SYS_PTRACE` + `--gpus all`
  （`--pid=host` 同时让 client 上报的 GPU 进程列表在 dashboard 上可见）

单机演示（server 与 client 起在同一台带 GPU 的机器上）时上述条件合并到一台。

---

## 1. 起栈（单机）

```bash
cd deploy/docker
docker compose up -d --build
```

首次 build：server ~5min，client ~15min。compose 里 client 走
`network_mode: host`，用 `SERVER_HOST=127.0.0.1:17000` 直连 server。

一键自检（起栈 → 等 client 注册 → 触发一次采集 → 拉报告）：

```bash
bash smoke.sh
# 或指定占 GPU 的 python 进程 PID
TARGET_PID=<pid> bash smoke.sh
```

---

## 2. 触发一次采集

在 server 机器执行：

```bash
S=127.0.0.1:17000      # 跨机时改为 <server-ip>:17000

# 2.1 确认 client 已注册（wsConnected=true 那条即可用）
curl -fsS "http://$S/api/clients" | python3 -m json.tool

# 2.2 触发 10s 采集（PID 换成 client 机器上占 GPU 的 python 进程）
curl -fsS -X POST "http://$S/api/v1/app_observ/aiAnalysis/start_ai_analysis" \
  -H 'Content-Type: application/json' \
  -d '{"instance":"<clientId>","pids":"<PID>","timeout":10000}'
# 返回 { "analysisId": "<UUID>", ... }
```

- `instance` = 步骤 2.1 里 `wsConnected=true` 的 clientId
- `timeout` 单位 ms
- 缺省同时启 gpu + pyki collector；target 上没跑 CUDA 会立刻返回
  `No process meets the conditions of the collector`

---

## 3. 打开报告

浏览器访问：

- Dashboard：<http://localhost:17000/aiprof>
- 直达报告页：<http://localhost:17000/api/v1/app_observ/aiAnalysis/report?analysisId=UUID>

页面上三大区块：

1. **元数据卡片** — analysisId、目标 PID / 实例名、采集时长、Peak RSS
2. **AI 分析结论** — 三种 LLM 通路，可切换、可展开/收起、有缓存时间戳
3. **原始数据视图** — 火焰图、Perfetto 时间线、内存快照、日志

---

## 4. AI 归因

结论卡片支持三条通路（同一份 profiling 可自由切换）：

| 通路 | mode | 何时用 |
|---|---|---|
| **本地 Agent** | `local` | 快速本地分析，无外部依赖 |
| **本地 OpenClaw** | `openclaw` | 需要多轮工具调用 / 深度归因 |
| **阿里云 Agent** | — | 「阿里云 Agent 分析 ↗」跳转外部 Demo 环境（`VITE_ALIYUN_AGENT_URL`） |

本地两条通路只需配置一个 OpenAI 兼容 LLM 的 Base URL + API Key（设置页或
`QWEN_API_KEY` 环境变量）。结论按 `analysisId` 落盘到
`<RESULT_DIR>/<analysisId>/ai_conclusion.json`，跨浏览器都能看到、重启
server 也不丢。

---

## 5. 跨机部署

```
server 机器 (无需 GPU)                     client 机器 (带 GPU)
┌───────────────────────┐                 ┌───────────────────────────┐
│ aiprof-server         │◀── ws://…/ws ───│ aiprof-client              │
│  dashboard+WS+分析     │  + HTTP upload  │  CollectionFramework       │
│  :17000               │                 │  --pid host --gpus all     │
└───────────────────────┘                 └───────────────────────────┘
```

构建镜像（context 必须是 repo 根）：

```bash
docker build -f deploy/docker/Dockerfile.server -t aiprof/server:latest .
docker build -f deploy/docker/Dockerfile.client -t aiprof/client:latest .
```

分发并起栈：

```bash
# server 机器
bash deploy/docker/run-server.sh --port 17000

# client 机器
bash deploy/docker/run-client.sh --server <server-ip>:17000 --client-id gpu-a10-01
```

跨机排错见 [`deployment.md`](deployment.md) §5 与
[`../deploy/docker/README-split.md`](../deploy/docker/README-split.md)。
