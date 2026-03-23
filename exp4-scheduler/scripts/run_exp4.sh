#!/usr/bin/env bash
# 实验 4：调度算法对比实验 —— 一键跑全部 workload 并输出结果
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "============================================"
echo "  实验 4：可插拔调度器对比实验"
echo "============================================"
echo ""
echo "运行 cargo test（三组 workload × 五种调度器）..."
echo ""

cd "$CRATE_DIR"
cargo test --test sched_compare -- --nocapture --test-threads=1 2>&1

echo ""
echo "============================================"
echo "  实验完成！上方表格即为对比结果"
echo "============================================"
echo ""
echo "指标说明："
echo "  AvgWait        — 平均等待时间（tick）"
echo "  AvgTurnaround  — 平均周转时间（tick）"
echo "  Throughput     — 吞吐量（任务数/tick）"
echo "  P95Lat / P99Lat — 交互延迟分位数（wakeup→dispatch，tick）"
echo "  Starvation     — 饥饿发生次数（单次连续等待 ≥ 阈值）"
