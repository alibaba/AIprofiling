# AIProf 容器化部署

两个镜像：

| 镜像 | 作用 | 端口 |
|---|---|---|
| `aiprof/server` | dashboardServer + analysis_summary + webapp | 17000 |
| `aiprof/client` | CollectionFramework + WS client（拉任务、注入 pyki、上传结果） | — |

## 快速起

```bash
cd <repo root>
docker compose -f deploy/docker/docker-compose.yml up -d --build
```

第一次 build：server ~5min（要 npm ci + PyInstaller），client ~15min（cargo release）。

**验证 server 起来了**：

```bash
curl -fsS http://127.0.0.1:17000/api/clients | jq
# 应该看到 aiprof-client-local 出现在列表里、状态 Idle
```

**触发一次 10s 采集**（PID 换成你要测的目标进程 —— 必须是 nvidia-smi 里 "GPU Memory Usage" 一栏有值的 python 进程）：

```bash
PID=$(nvidia-smi --query-compute-apps=pid --format=csv,noheader | head -1)
curl -sS -X POST http://127.0.0.1:17000/api/v1/app_observ/aiAnalysis/start_ai_analysis \
  -H 'Content-Type: application/json' \
  -d "{\"instance\":\"aiprof-client-local\",\"pids\":\"${PID}\",\"timeout\":10000}"
# 返回 { "analysisId": "<UUID>", ... }
```

> `instance` = clientId（对应 `/api/clients` 里 wsConnected=true 那条的 clientId）
> `timeout` 单位 ms
> 缺省会同时启 gpu + pyki collector；若 target 上没跑 CUDA，会立刻返回 `No process meets the conditions of the collector`

10 秒后 client 会 tar 上传，server 跑 analysis_summary。**打开报告**：

```
http://<host>:17000/api/v1/app_observ/aiAnalysis/report?analysisId=<UUID>
```

## 宿主要求

**必需**：
- Linux ≥ 5.8
- Docker Engine ≥ 24 + `docker compose` v2
- NVIDIA GPU + [nvidia-container-toolkit](https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/latest/install-guide.html)（`--gpus all` 生效）

**client 权限**（compose 里已配好）：
- `pid: host` — pyki 要 ptrace 注入宿主目标进程
- `--cap-add SYS_PTRACE SYS_ADMIN`
- `seccomp:unconfined` — ptrace syscall 常规 seccomp 会拦

**如果目标进程也在容器里跑**：
- compose 里把 `aiprof-client` 的 `pid: host` 改成 `pid: "container:<target-container-name>"`
- ptrace 目标进程时不再需要 pid=host，但仍需 `CAP_SYS_PTRACE`

## Vendored .so 说明

client 镜像里 `/opt/aiprof/` 下平铺了一堆预编译 `.so`：

| 库 | 用途 | 来源 |
|---|---|---|
| `libloader.so` | pyki injector（ptrace 目标进程注入 `libpython`） | `agent/collection_framework/src/plugins/pykiLoader/` |
| `libcuprof.so` | CUDA kernel trace（CUPTI 时间线） | `agent/collection_framework/src/plugins/cuprof/`（构建时由 `make` 产出） |
| `cupti/libcupti.so.*` | CUPTI runtime，供 `libcuprof.so` dlopen | `agent/collection_framework/src/third_party/cupti/` |

**Host libcuda 覆盖**：nvidia-container-toolkit 会把宿主 `/usr/lib/x86_64-linux-gnu/libcuda.so.*` 挂进容器 `/usr/lib64`，`LD_LIBRARY_PATH` 里那个路径优先级高于镜像自带的 `libcuda.so.535.161.07`，跨驱动版本的兼容性由宿主保证，容器不用管。

## 常见问题

| 症状 | 原因 & 处置 |
|---|---|
| `curl /api/clients` 里看不到 client | server 里 WS `/ws` 没连上。看 client 日志：`docker logs aiprof-client`。多半是 `SERVER_HOST` 环境变量没走宿主网络 → 检查 compose 里 `network_mode: host` 是否生效 |
| client 起不来，报 `cannot open shared object` | `LD_LIBRARY_PATH` 没生效。用 `docker exec aiprof-client env \| grep LD_LIBRARY` 确认；或用 `ldd /opt/aiprof/CollectionFramework` 看哪个 so 找不到 |
| 采集触发后 client 日志 `No processes meet the resource requirements` | 目标 PID 不存在、或没在跑 CUDA/torch。CF detector 通过 `nvidia-smi --query-compute-apps=pid` 校验 |
| 采集完了但报告页 `无 kernel 数据` | 目标进程没用 CUDA kernel，或 CUPTI 附加失败。看 client 日志里 `Handling event: StartCollector(Pyki, ...)` 之后 30 秒内的 warn |
| Perfetto 页打不开 trace | 宿主/浏览器要能访问 `ui.perfetto.dev`（公网）。不在报告 URL 层，前端 iframe 走公网 |

## 快速验证脚本

跑一次自检（server 起、client 注册、伪采集触发、报告 URL 返回 200）：

```bash
bash deploy/docker/smoke.sh   # 见下一节
```

## 只 build 一个镜像

```bash
docker build -f deploy/docker/Dockerfile.server -t aiprof/server:latest .
docker build -f deploy/docker/Dockerfile.client -t aiprof/client:latest .
```

（从 repo root 执行，`context` 必须是根，两个 Dockerfile 都会拷 `agent/`、`server/`、`ui/`。）

## 未做

- 推 image 到公开 registry（用户自己控 credential）
- k8s manifest（在 `deploy/_legacy/k8s/` 保留了旧版；两镜像跑通后再单独提新版）
- 如需 NVTX / NCCL / RDMA / DCGM / ROCm 等更丰富指标，请到阿里云操作系统控制台使用
