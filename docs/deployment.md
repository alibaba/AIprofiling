# AIProf Docker 部署指导

AIProf 以两个镜像交付：

| 镜像 | 作用 | 端口 |
|---|---|---|
| `aiprof/server` | dashboardServer.js + analysis_summary + webapp（报告页/API/WS/上传） | 17000 |
| `aiprof/client` | CollectionFramework + WS agent（注入 pyki、采集、上传结果） | — |

client 通过 WebSocket 连到 server 拉任务，采集完把结果打包经 HTTP 上传回 server；
server 跑分析并出报告。三种部署形态：

1. **单机 compose** —— 最省事，一条命令起两个容器（见 §2）。
2. **单机双容器脚本** —— 用 `run-server.sh` / `run-client.sh`，参数可调（见 §3）。
3. **跨机部署** —— server 与带 GPU 的 client 分处两台机器（见 §4）。

---

## 1. 前置要求

**server 机器**（可无 GPU）：
- Docker Engine ≥ 24
- 对 client 放行入方向端口 17000（WS 与上传共用此端口）

**client 机器**（必须有 GPU）：
- NVIDIA GPU + 驱动 + [nvidia-container-toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)（`--gpus all` 生效）
- 能 ptrace 目标进程 → 容器需 `--pid=host` + `CAP_SYS_PTRACE`
  （`--pid=host` 同时让 client 心跳里的 GPU 进程列表在 dashboard 上可见；
  不开的话 `nvidia-smi --query-compute-apps` 在容器 pid ns 里返回空）

> 验证 GPU 容器可用：`docker run --rm --gpus all nvidia/cuda:12.2.0-runtime-ubuntu22.04 nvidia-smi`

---

## 2. 单机 compose（最快）

```bash
cd <repo root>
docker compose -f deploy/docker/docker-compose.yml up -d --build
```

首次 build：server ~5min（npm ci + PyInstaller），client ~15min（cargo release）。
compose 里 client 走 `network_mode: host`，用 `SERVER_HOST=127.0.0.1:17000` 直连 server。

一键自检（起栈 → 等注册 → 触发采集 → 拉报告）：

```bash
bash deploy/docker/smoke.sh
# 若宿主已有占 GPU 的进程会自动挑一个；否则指定：
TARGET_PID=<pid> bash deploy/docker/smoke.sh
```

---

## 3. 单机双容器脚本（参数可调）

不想用 compose、或想改端口/卷/clientId 时用脚本。两台都是本机：

```bash
cd <repo root>
bash deploy/docker/run-server.sh --build --port 17000
bash deploy/docker/run-client.sh --build --server 127.0.0.1:17000 --client-id local-gpu
```

参数详见各脚本 `--help`，也见 §4 的表格（跨机与单机参数一致，只是 `--server` 填的地址不同）。

---

## 4. 跨机部署

```
server 机器 (无需 GPU)                     client 机器 (带 GPU)
┌───────────────────────┐                 ┌───────────────────────────┐
│ aiprof-server          │◀── ws://…/ws ───│ aiprof-client              │
│  dashboard+WS+分析      │   + HTTP upload  │  CollectionFramework       │
│  :17000                │                 │  --pid host --gpus all     │
└───────────────────────┘                 └───────────────────────────┘
```

> 结果通过 HTTP multipart 上传回 server 落盘，两台机器**不需要**共享卷。

### 4.1 构建镜像

从 repo root 执行（构建 context 必须是根，两个 Dockerfile 都会拷 `agent/` `server/` `ui/`）：

```bash
docker build -f deploy/docker/Dockerfile.server -t aiprof/server:latest .
docker build -f deploy/docker/Dockerfile.client -t aiprof/client:latest .
```

分发到目标机器（有私有 registry 就 push/pull；没有可 `docker save | ssh … docker load`）：

```bash
docker save aiprof/server:latest | ssh root@<server-ip> 'docker load'
docker save aiprof/client:latest | ssh root@<client-ip> 'docker load'
```

### 4.2 server 机器

```bash
bash deploy/docker/run-server.sh --port 17000
```

| 参数 | 环境变量 | 默认 | 说明 |
|---|---|---|---|
| `--image` | `IMAGE` | `aiprof/server:latest` | 镜像 tag |
| `--name` | `NAME` | `aiprof-server` | 容器名 |
| `--port` | `PORT` | `17000` | 对外端口 |
| `--bind` | `BIND` | `0.0.0.0` | 监听地址；只本机访问填 `127.0.0.1` |
| `--data` | `DATA_VOLUME` | `aiprof-data` | 结果/上传落盘卷或宿主目录 |
| `--restart` | `RESTART` | `unless-stopped` | 重启策略 |
| `--build` | `BUILD` | 关 | 起容器前先 build |

### 4.3 client 机器（带 GPU）

跨机唯一必改的是 `--server`：

```bash
bash deploy/docker/run-client.sh \
  --server <server-ip>:17000 \
  --client-id gpu-a10-01
```

| 参数 | 环境变量 | 默认 | 说明 |
|---|---|---|---|
| `--server` | `SERVER_HOST` | `127.0.0.1:17000` | **server 的 host:port**（跨机必改） |
| `--client-id` | `CLIENT_ID` | 主机名 | 注册到 server 的 clientId |
| `--image` | `IMAGE` | `aiprof/client:latest` | 镜像 tag |
| `--name` | `NAME` | `aiprof-client` | 容器名 |
| `--gpus` | `GPUS` | `all` | `--gpus` 值，如 `'"device=0"'` |
| `--target` | `TARGET_CONTAINER` | 空 | 目标进程所在容器名；填了走 `pid:container:<name>`，否则 `pid host` |
| `--transport` | `TRANSPORT` | `ws` | 控制平面传输；反代剥掉 WS `Upgrade` 头时设 `poll` 走 HTTP long-poll |
| `--restart` | `RESTART` | `unless-stopped` | 重启策略 |
| `--privileged` | `PRIVILEGED` | 关 | 用 `--privileged`（想省事一把授权时用） |
| `--build` | `BUILD` | 关 | 起容器前先 build |

> **代理剥 Upgrade 头时用 poll**：server 若部署在会剥掉 `Connection: Upgrade` /
> `Upgrade: websocket` 头的反向代理（SLB/WAF/网关）后面，WS 握手到达容器时退化成普通
> GET，`/ws` 返回 404，client 永远注册不上。此时给 client 设 `--transport poll`
> （或 `-e TRANSPORT=poll`）走 HTTP long-poll 备用通路，`--server` 用代理入口
> `https://<gateway-host>/<prefix>`。trace 上传本就走 HTTP，不受影响。

---

## 5. 端到端验证（在 server 机器执行）

```bash
S=<server-ip>:17000   # 单机填 127.0.0.1:17000

# a. client 注册成功？应看到你的 clientId、wsConnected=true
curl -fsS "http://$S/api/clients" | python3 -m json.tool

# b. 触发一次 10s 采集（PID 换成 client 机器上占 GPU 的 python 进程）
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

## 6. 排错

| 症状 | 原因 & 处置 |
|---|---|
| `/api/clients` 里看不到 client | WS `/ws` 没连上。`docker logs -f aiprof-client` 看日志；跨机多半 `--server`/`SERVER_HOST` 写错或安全组没放行端口；单机 compose 确认 `network_mode: host` 生效 |
| client 起不来，报 `…so: cannot open shared object` | `LD_LIBRARY_PATH` 没生效。`docker exec aiprof-client env \| grep LD_LIBRARY` 确认，或 `ldd /opt/aiprof/CollectionFramework` 看缺哪个 so |
| 采集触发后 `No processes meet the resource requirements` | 目标 PID 不存在或没跑 CUDA/torch。CF 通过 `nvidia-smi --query-compute-apps=pid` 校验；进程在别的容器里用 `--target <容器名>` |
| 采集完但报告页「无 kernel 数据」 | 目标没用 CUDA kernel，或 CUPTI 附加失败。看 client 日志里 `StartCollector(Pyki, …)` 之后 30s 内的 warn |
| 上传失败 / 报告不生成 | 确认 server 端口对 client **出方向**可达；大 trace 走 HTTP，双向可达最稳 |
| Perfetto 页打不开 trace | 前端 iframe 走公网 `ui.perfetto.dev`，浏览器需能访问公网 |

---

## 7. AI 分析后端（三种通路）

报告页「AI 分析结论」卡片支持三种 LLM 通路，前端按钮：

| 通路 | mode | 说明 |
|---|---|---|
| **本地 Agent** | `local` | server 侧 `local-profiling-agent`：解析 chrome-tracing → 固定指标工具 → LLM 多轮 refine 综合结论。走任一 OpenAI 兼容 LLM，Key 只在本次请求转发到 server，不落盘。 |
| **本地 OpenClaw** | `openclaw` | server 侧 spawn `openclaw agent --local` 一次性子进程，走 agentic 循环 + MCP 工具。Key 只进子进程环境，不落盘。 |
| **阿里云 Agent** | — | 「阿里云 Agent 分析 ↗」按钮跳转外部 Demo 环境（`VITE_ALIYUN_AGENT_URL`，默认 `https://soma.openanolis.cn`）。云端深度诊断不在开源范围内，通过外链体验。 |

### 7.1 阿里云 Agent 外链
「阿里云 Agent 分析」按钮为纯外链跳转，不调用任何云端分析 OpenAPI。跳转地址由前端 `VITE_ALIYUN_AGENT_URL`（构建期）/ server 侧 `ALIYUN_AGENT_URL`（错误提示用）配置，默认 `https://soma.openanolis.cn`。详见 `docs/design-aliyun-agent-redirect.md`。

### 7.2 本地 OpenClaw
两种安装方式，二选一：

**A. 镜像预装（推荐生产/离线）**
```bash
docker build -f deploy/docker/Dockerfile.server \
  --build-arg WITH_OPENCLAW=1 \
  -t aiprof/server:with-openclaw .
```
构建期执行 `npm install -g openclaw && openclaw plugins install @openclaw/qwen-provider`。镜像体积增加约 200MB。

**B. 运行时安装（默认，按需触发）**
镜像不预装。用户第一次点「本地 OpenClaw 分析」时弹窗展示「一键安装」按钮，前端调用：
- `GET /api/ai/openclaw/status` — 探测二进制 + Qwen 插件，返回安装/进度状态。
- `POST /api/ai/openclaw/install` — 后台 `npm install -g --prefix $PERSISTENCE_DIR/openclaw openclaw`，装完追装 `@openclaw/qwen-provider`。进度轮询同一个 status 接口。
- 安装成功后 sentinel 落在 `$PERSISTENCE_DIR/openclaw/.installed`，重启不丢（因为 `$PERSISTENCE_DIR` 挂在 `/var/lib/aiprof` 卷上）。

> 安装装到 server 私有目录（`NPM_CONFIG_PREFIX=$PERSISTENCE_DIR/openclaw`），不污染宿主机 npm 全局。宿主机部署时同一路径要求 server 进程对 `$PERSISTENCE_DIR` 可写（默认成立）。

**运行时环境变量：**
- `QWEN_API_KEY` — 可选。设置后前端弹窗内的 API Key 输入框可留空（server 兜底）。
- `AIPROF_OPENCLAW_MODEL` — 默认 `qwen/qwen3.5-plus`。前端弹窗内可覆盖。
- `AIPROF_OPENCLAW_TIMEOUT_S` — openclaw 单次 turn 超时秒，默认 `300`。
- `AIPROF_OPENCLAW_REGISTRY` — 默认 `https://registry.npmjs.org`（openclaw 只发到官方源；npmmirror 上同名的是 0.0.1 空壳，会导致安装成功但没有 `openclaw` 可执行）。

> **Node 版本要求**：openclaw 要求 Node `>=22.22.3 <23 || >=24.15.0 <25 || >=25.9.0`。aiprof/server 镜像基于 `node:22-bookworm-slim`，宿主机手动装时也需先 `nvm install 24` 之类。

**手动兜底（安装失败时）：**
```bash
# 宿主机：npm i -g openclaw && openclaw plugins install @openclaw/qwen-provider
# 容器：docker exec -it aiprof-server sh -lc \
#   'npm i -g --prefix /var/lib/aiprof/state/openclaw openclaw \
#    && /var/lib/aiprof/state/openclaw/bin/openclaw plugins install @openclaw/qwen-provider'
```

### 7.3 直连 LLM（自定义）
无需服务端配置。用户点「自定义 LLM 分析」在弹窗内填入即可。

---

## 8. 相关文件

| 文件 | 说明 |
|---|---|
| `deploy/docker/Dockerfile.server` | server 镜像（多阶段：webapp → analysis_summary → node runtime；`--build-arg WITH_OPENCLAW=1` 预装 OpenClaw） |
| `deploy/docker/Dockerfile.client` | client 镜像（Rust build → cuda runtime + pyki + vendored .so） |
| `deploy/docker/docker-compose.yml` | 单机两容器栈 |
| `deploy/docker/run-server.sh` / `run-client.sh` | 参数化启动脚本 |
| `deploy/docker/smoke.sh` | 单机端到端自检 |
| `deploy/docker/README.md` | 单机 compose 详解 + vendored .so 说明 |
| `deploy/docker/README-split.md` | 跨机部署速查 |
