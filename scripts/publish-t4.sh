#!/usr/bin/env bash
# Publish the T4 crate chain to crates.io in dependency order.
# Usage:
#   bash scripts/publish-t4.sh dry-run
#   bash scripts/publish-t4.sh publish

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:-dry-run}"
WAIT_SECS="${WAIT_SECS:-15}"

if [[ "$MODE" != "dry-run" && "$MODE" != "publish" ]]; then
    echo "usage: bash scripts/publish-t4.sh [dry-run|publish]"
    exit 1
fi

if [[ "$MODE" == "dry-run" ]]; then
    CARGO_CMD=(cargo package --allow-dirty --list)
else
    CARGO_CMD=(cargo publish --allow-dirty)
fi

CRATES=(
    "tg-rcore-tutorial-sbi"
    "tg-rcore-tutorial-task-manage"
    "exp4-scheduler"
    "exp5-sync"
    "syscall-t3l8"
    "tg-rcore-tutorial-user"
    "tg-rcore-tutorial-ch8"
)

echo "mode: $MODE"
echo "root: $ROOT"

for crate_dir in "${CRATES[@]}"; do
    echo
    echo "==> ${crate_dir}"
    (
        cd "$ROOT/$crate_dir"
        "${CARGO_CMD[@]}"
    )
    if [[ "$MODE" == "publish" ]]; then
        echo "waiting ${WAIT_SECS}s for crates.io index to catch up..."
        sleep "$WAIT_SECS"
    fi
done

echo
echo "T4 publish flow finished."
