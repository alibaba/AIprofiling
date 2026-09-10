#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# 在 server 机器上起 aiprof-server（dashboard + WS + analysis + webapp）。
# 所有可变项都做成参数，默认值见下方。
#
#   bash run-server.sh                 # 用默认值
#   PORT=18000 bash run-server.sh      # 环境变量覆盖
#   bash run-server.sh --port 18000    # 命令行覆盖（优先级最高）
#
set -euo pipefail

# ── 默认值（可用环境变量覆盖）─────────────────────────────
IMAGE="${IMAGE:-aiprof/server:latest}"       # 镜像 tag
NAME="${NAME:-aiprof-server}"                 # 容器名
PORT="${PORT:-17000}"                         # 对外端口（宿主:容器 一致）
BIND="${BIND:-0.0.0.0}"                       # 监听地址；只想本机访问填 127.0.0.1
DATA_VOLUME="${DATA_VOLUME:-aiprof-data}"     # 结果/上传落盘卷（named volume 或宿主目录）
RESTART="${RESTART:-unless-stopped}"          # docker restart policy
BUILD="${BUILD:-0}"                           # 1=先 build 镜像

usage() {
  cat <<EOF
用法: bash run-server.sh [选项]
  --image <tag>        镜像 tag              (默认 $IMAGE)
  --name <name>        容器名                (默认 $NAME)
  --port <port>        对外端口              (默认 $PORT)
  --bind <addr>        监听地址              (默认 $BIND，公网放行需 0.0.0.0)
  --data <vol|dir>     数据卷/目录           (默认 $DATA_VOLUME)
  --restart <policy>   重启策略              (默认 $RESTART)
  --build              起容器前先 build 镜像
  -h, --help           显示本帮助
EOF
}

# ── 解析命令行（覆盖环境变量）───────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --image)   IMAGE="$2"; shift 2;;
    --name)    NAME="$2"; shift 2;;
    --port)    PORT="$2"; shift 2;;
    --bind)    BIND="$2"; shift 2;;
    --data)    DATA_VOLUME="$2"; shift 2;;
    --restart) RESTART="$2"; shift 2;;
    --build)   BUILD=1; shift;;
    -h|--help) usage; exit 0;;
    *) echo "未知参数: $1" >&2; usage; exit 1;;
  esac
done

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [ "$BUILD" = "1" ]; then
  echo "==> build $IMAGE"
  docker build -f "$REPO_ROOT/deploy/docker/Dockerfile.server" -t "$IMAGE" "$REPO_ROOT"
fi

# 已存在同名容器先清掉，避免 name 冲突
docker rm -f "$NAME" >/dev/null 2>&1 || true

echo "==> run $NAME  ($BIND:$PORT -> 17000)"
docker run -d --name "$NAME" --restart "$RESTART" \
  -p "$BIND:$PORT:17000" \
  -v "$DATA_VOLUME:/var/lib/aiprof" \
  "$IMAGE"

echo
echo "✅ server 已启动"
echo "   健康检查:  curl -fsS http://127.0.0.1:$PORT/api/clients"
echo "   报告首页:  http://<本机IP>:$PORT/aiprof"
echo "   client 端 SERVER_HOST 应填:  <本机IP>:$PORT"
