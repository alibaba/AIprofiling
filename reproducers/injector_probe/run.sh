#!/usr/bin/env bash
# Convenience runner: spawn target.py, run probe.c against it,
# tear down cleanly.
#
# Usage:
#   ./run.sh <mode> [extra probe args...]
#
# Modes:
#   tls          — target dlopen/dlclose churn
#   heap         — target malloc/free churn
#   mixed        — both
#   torch        — minimal torch loop with record_memory_history
#
# Examples:
#   ./run.sh tls                          # default: 200 iters, timeout=30
#   ./run.sh heap --iterations 500
#   ./run.sh mixed --timeout 0            # reproduce wait4-hangs-forever
#   ./run.sh tls --detach-on-fail         # reproduce double-free crash

set -euo pipefail
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
cd "$SCRIPT_DIR"

MODE="${1:-mixed}"
shift || true

if [ ! -x ./probe ]; then
    echo "probe binary not built. run: make LIBPROFILER_DIR=..." >&2
    exit 2
fi

PYTHON="${PYTHON:-python3}"
$PYTHON target.py --mode "$MODE" &
TARGET_PID=$!
trap 'kill -9 $TARGET_PID 2>/dev/null || true' EXIT

# Give target time to spin up worker threads.
sleep 1

./probe --pid "$TARGET_PID" "$@"
