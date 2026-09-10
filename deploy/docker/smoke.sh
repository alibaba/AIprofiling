#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# AIProf 两镜像栈自检：起容器 → 等 healthy → 用 stub PID 触发 → 拉报告
# 依赖：docker compose v2、curl、python3（用来解 JSON，避免依赖 jq）
set -euo pipefail

# 用 python3 解 JSON，避免让用户装 jq
json_get() { python3 -c "import sys,json;d=json.load(sys.stdin);
try:
    for k in '$1'.split('.'):
        d=d[int(k)] if k.isdigit() else d[k]
    print(d)
except Exception: pass"; }
count_clients() { python3 -c "import sys,json;print(len(json.load(sys.stdin).get('data',[])))"; }
first_client_id() { python3 -c "
import sys,json
data=json.load(sys.stdin).get('data',[])
prefer=[c for c in data if c.get('clientId')=='aiprof-client-local' and c.get('wsConnected')]
pick=(prefer or [c for c in data if c.get('wsConnected')] or data)
print(pick[0]['clientId'] if pick else '')"; }

COMPOSE="docker compose -f $(dirname "$0")/docker-compose.yml"

echo "==> 拉起 aiprof-server + aiprof-client"
$COMPOSE up -d --build

echo "==> 等 server healthy（最多 60s）"
for i in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:17000/api/clients >/dev/null 2>&1; then
    echo "    ok"
    break
  fi
  sleep 2
  [ "$i" -eq 30 ] && { echo "server 起不来"; $COMPOSE logs aiprof-server | tail -30; exit 1; }
done

echo "==> 等 client 注册（最多 30s）"
for i in $(seq 1 15); do
  n=$(curl -fsS http://127.0.0.1:17000/api/clients 2>/dev/null | count_clients 2>/dev/null || echo 0)
  [ "$n" -gt 0 ] && { echo "    ok（$n 个 client）"; break; }
  sleep 2
  [ "$i" -eq 15 ] && { echo "client 没注册"; $COMPOSE logs aiprof-client | tail -30; exit 1; }
done

CLIENT_ID=$(curl -fsS http://127.0.0.1:17000/api/clients | first_client_id)
echo "==> 使用 clientId=$CLIENT_ID"

# 拿一个真正持有 CUDA context 的 pid（pgrep python 会抓到 pgrep 自己派生的子进程，转瞬即逝）
TARGET_PID="${TARGET_PID:-$(nvidia-smi --query-compute-apps=pid --format=csv,noheader 2>/dev/null | head -1 | tr -d ' ' || true)}"
if [ -z "$TARGET_PID" ]; then
  echo "!! 宿主 GPU 上没有活跃 CUDA 进程（nvidia-smi 查不到），跳过采集验证"
  echo "   起一个 torch workload 后再跑：TARGET_PID=<pid> $0"
  exit 0
fi
echo "==> 触发 10s 采集 pid=$TARGET_PID"
RESP=$(curl -fsS -X POST http://127.0.0.1:17000/api/v1/app_observ/aiAnalysis/start_ai_analysis \
  -H 'Content-Type: application/json' \
  -d "{\"instance\":\"$CLIENT_ID\",\"pids\":\"$TARGET_PID\",\"timeout\":10000}")
AID=$(echo "$RESP" | python3 -c "import sys,json;d=json.load(sys.stdin);print(d.get('analysisId') or d.get('data',{}).get('analysisId',''))")
echo "    analysisId=$AID"

echo "==> 等 report 生成（最多 90s，含 tar 上传 + analysis）"
for i in $(seq 1 45); do
  code=$(curl -s -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:17000/api/v1/app_observ/aiAnalysis/report?analysisId=$AID")
  [ "$code" = "200" ] && { echo "    ok（HTTP 200）"; break; }
  sleep 2
  [ "$i" -eq 45 ] && { echo "报告没生成"; $COMPOSE logs aiprof-server | tail -30; exit 1; }
done

echo
echo "✅ 全链路 OK"
echo "报告 URL: http://127.0.0.1:17000/api/v1/app_observ/aiAnalysis/report?analysisId=$AID"
