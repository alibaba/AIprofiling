#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# 在带 GPU 的 client 机器上起 aiprof-client（CollectionFramework + WS agent）。
# client 通过 WS 连到 server 拉任务，采集后把结果 tar 经 HTTP 上传回 server。
# 跨机部署时唯一必须改的就是 --server / SERVER_HOST。
#
#   SERVER_HOST=<SERVER_IP>:17000 bash run-client.sh
#   bash run-client.sh --server <SERVER_IP>:17000 --client-id gpu-a10-01
#
# 若 server 在会剥掉 WS `Upgrade` 头的反向代理后面（WS 握手退化成普通 GET），
# 加 --transport poll 走 HTTP long-poll 备用通路（trace 上传本就走 HTTP，不受影响）：
#
#   bash run-client.sh --server https://<gateway-host>/<prefix> --transport poll --client-id gpu-a10-01
#
set -euo pipefail

# ── 默认值（可用环境变量覆盖）─────────────────────────────
IMAGE="${IMAGE:-aiprof/client:latest}"        # 镜像 tag
NAME="${NAME:-aiprof-client}"                  # 容器名
SERVER_HOST="${SERVER_HOST:-127.0.0.1:17000}"  # server 的 host:port（跨机改成 server IP）
CLIENT_ID="${CLIENT_ID:-$(hostname)}"          # 注册到 server 的 clientId（缺省=主机名）
GPUS="${GPUS:-all}"                            # --gpus 值：all / '"device=0"' 等
TARGET_CONTAINER="${TARGET_CONTAINER:-}"       # 若目标进程在某容器里：填其容器名，走 pid:container
RESTART="${RESTART:-unless-stopped}"           # docker restart policy
PRIVILEGED="${PRIVILEGED:-0}"                  # 1=用 --privileged（内核<5.8 退化路径）
BUILD="${BUILD:-0}"                            # 1=先 build 镜像
TRANSPORT="${TRANSPORT:-}"                     # 控制平面传输：空=ws(默认) / poll(反代剥 Upgrade 头时用)

usage() {
  cat <<EOF
用法: bash run-client.sh [选项]
  --server <host:port>    server 地址（必填/最常改）  (默认 $SERVER_HOST)
  --client-id <id>        注册用 clientId             (默认 主机名)
  --image <tag>           镜像 tag                    (默认 $IMAGE)
  --name <name>           容器名                      (默认 $NAME)
  --gpus <spec>           --gpus 值                   (默认 $GPUS)
  --target <container>    目标进程所在容器名(pid共享)  (默认 空=pid host)
  --restart <policy>      重启策略                    (默认 $RESTART)
  --transport <ws|poll>   控制平面传输，反代剥 WS Upgrade 头时用 poll (默认 ws)
  --privileged            用 --privileged（内核<5.8）
  --build                 起容器前先 build 镜像
  -h, --help              显示本帮助
EOF
}

# ── 解析命令行（覆盖环境变量）───────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --server)     SERVER_HOST="$2"; shift 2;;
    --client-id)  CLIENT_ID="$2"; shift 2;;
    --image)      IMAGE="$2"; shift 2;;
    --name)       NAME="$2"; shift 2;;
    --gpus)       GPUS="$2"; shift 2;;
    --target)     TARGET_CONTAINER="$2"; shift 2;;
    --restart)    RESTART="$2"; shift 2;;
    --transport)  TRANSPORT="$2"; shift 2;;
    --privileged) PRIVILEGED=1; shift;;
    --build)      BUILD=1; shift;;
    -h|--help)    usage; exit 0;;
    *) echo "未知参数: $1" >&2; usage; exit 1;;
  esac
done

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [ "$BUILD" = "1" ]; then
  echo "==> build $IMAGE"
  docker build -f "$REPO_ROOT/deploy/docker/Dockerfile.client" -t "$IMAGE" "$REPO_ROOT"
fi

# PID namespace：默认共享宿主(pid host)；指定 --target 则共享那个容器
if [ -n "$TARGET_CONTAINER" ]; then
  PID_ARG=(--pid "container:$TARGET_CONTAINER")
  echo "==> 目标进程在容器 $TARGET_CONTAINER 内，共享其 PID namespace"
else
  PID_ARG=(--pid host)
fi

# 权限：默认按需 cap；想省事可用 --privileged
if [ "$PRIVILEGED" = "1" ]; then
  PRIV_ARG=(--privileged)
else
  PRIV_ARG=(
    --cap-add SYS_PTRACE --cap-add SYS_ADMIN
    --security-opt seccomp=unconfined --security-opt apparmor=unconfined
  )
fi

docker rm -f "$NAME" >/dev/null 2>&1 || true

# Only pass TRANSPORT when set, so an unset value leaves the client on its
# default ws transport (empty -e TRANSPORT= would override to "" and confuse
# the client's ws/poll check).
TRANSPORT_ARG=()
if [ -n "$TRANSPORT" ]; then
  TRANSPORT_ARG=(-e TRANSPORT="$TRANSPORT")
fi

echo "==> run $NAME  (SERVER_HOST=$SERVER_HOST  CLIENT_ID=$CLIENT_ID  gpus=$GPUS  transport=${TRANSPORT:-ws})"
docker run -d --name "$NAME" --restart "$RESTART" \
  "${PID_ARG[@]}" --ipc host \
  --gpus "$GPUS" \
  "${PRIV_ARG[@]}" \
  -e SERVER_HOST="$SERVER_HOST" \
  -e CLIENT_ID="$CLIENT_ID" \
  "${TRANSPORT_ARG[@]}" \
  -e RUST_LOG="${RUST_LOG:-info}" \
  "$IMAGE"

echo
echo "✅ client 已启动"
echo "   看日志:      docker logs -f $NAME"
echo "   在 server 侧确认注册:  curl -fsS http://$SERVER_HOST/api/clients"
