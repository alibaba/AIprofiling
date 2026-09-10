# AIProf 跨机部署（client / server 分开）

`docker-compose.yml` 是**单机**两容器栈（走 loopback）。要把 server 和带 GPU 的
client 拆到**两台机器**，用这里的 `run-server.sh` / `run-client.sh`——所有
IP、端口、clientId 等都是参数，不用改镜像。

```
server 机器 (无需 GPU)                     client 机器 (带 GPU)
┌───────────────────────┐                 ┌───────────────────────────┐
│ aiprof-server          │◀── ws://…/ws ───│ aiprof-client              │
│  dashboard + WS + 分析 │   + HTTP upload  │  CollectionFramework + WS │
│  :17000                │                 │  --pid host --gpus all     │
└───────────────────────┘                 └───────────────────────────┘
```

> 采集结果通过 HTTP multipart 上传回 server 落盘，两台机器**不需要**共享卷。

## 1. server 机器

```bash
cd <repo root>
bash deploy/docker/run-server.sh --build            # 首次带 --build
# 或已有镜像：
bash deploy/docker/run-server.sh --port 17000
```

参数（命令行 > 环境变量 > 默认）：

| 参数 | 环境变量 | 默认 | 说明 |
|---|---|---|---|
| `--image` | `IMAGE` | `aiprof/server:latest` | 镜像 tag |
| `--name` | `NAME` | `aiprof-server` | 容器名 |
| `--port` | `PORT` | `17000` | 对外端口 |
| `--bind` | `BIND` | `0.0.0.0` | 监听地址；只本机访问填 `127.0.0.1` |
| `--data` | `DATA_VOLUME` | `aiprof-data` | 结果/上传落盘卷或宿主目录 |
| `--restart` | `RESTART` | `unless-stopped` | 重启策略 |
| `--build` | `BUILD` | 关 | 起容器前先 build |

> **防火墙/安全组**：server 机器要对 client 机器 IP 放行 `--port`（WS 和上传共用这一个端口）。

## 2. client 机器（带 GPU）

只有 `--server` 是几乎每次都要改的：

```bash
cd <repo root>
bash deploy/docker/run-client.sh --build \
  --server <SERVER_IP>:17000 \
  --client-id gpu-a10-01
```

参数：

| 参数 | 环境变量 | 默认 | 说明 |
|---|---|---|---|
| `--server` | `SERVER_HOST` | `127.0.0.1:17000` | **server 的 host:port**（跨机必改） |
| `--client-id` | `CLIENT_ID` | 主机名 | 注册到 server 的 clientId |
| `--image` | `IMAGE` | `aiprof/client:latest` | 镜像 tag |
| `--name` | `NAME` | `aiprof-client` | 容器名 |
| `--gpus` | `GPUS` | `all` | `--gpus` 值，如 `'"device=0"'` |
| `--target` | `TARGET_CONTAINER` | 空 | 目标进程所在容器名；填了走 `pid:container:<name>`，否则 `pid host` |
| `--restart` | `RESTART` | `unless-stopped` | 重启策略 |
| `--privileged` | `PRIVILEGED` | 关 | 用 `--privileged`（内核 <5.8 退化路径） |
| `--build` | `BUILD` | 关 | 起容器前先 build |

## 3. 端到端验证（在 server 机器执行）

```bash
S=<SERVER_IP>:17000

# a. client 注册成功？应看到 gpu-a10-01、wsConnected=true
curl -fsS "http://$S/api/clients" | python3 -m json.tool

# b. 触发一次 10s 采集（PID 换成 client 机器上占 GPU 的 python 进程）
curl -fsS -X POST "http://$S/api/v1/app_observ/aiAnalysis/start_ai_analysis" \
  -H 'Content-Type: application/json' \
  -d '{"instance":"gpu-a10-01","pids":"<PID>","timeout":10000}'
# 返回 { "analysisId": "<UUID>", ... }

# c. 报告
#   http://<SERVER_IP>:17000/aiprof
#   http://<SERVER_IP>:17000/api/v1/app_observ/aiAnalysis/report?analysisId=<UUID>
```

## 排错

| 症状 | 处置 |
|---|---|
| `/api/clients` 里没有 client | `docker logs -f aiprof-client` 看 WS 连接；多半 `--server` 写错或安全组没放行端口 |
| client 连上但采集失败 | 目标 PID 必须在 client 宿主可见（`--pid host`）；进程若在别的容器里用 `--target <容器名>` |
| 上传失败 / 报告不生成 | 确认 server 端口对 client **出方向**可达；大 trace 走 HTTP，双向可达最稳 |
| 内核 <5.8 起不来 | client 加 `--privileged` |

宿主内核 / 权限 / vendored .so 等通用要求见同目录 `README.md`。
