#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== exp5-sync: 同步互斥机制实验 ==="
echo ""
echo "Running from: $CRATE_DIR"
echo ""

cd "$CRATE_DIR"

echo "--- cargo check ---"
cargo check 2>&1
echo ""

echo "--- Running all sync tests ---"
cargo test --test sync_compare -- --nocapture --test-threads=1 2>&1

echo ""
echo "=== Done ==="
