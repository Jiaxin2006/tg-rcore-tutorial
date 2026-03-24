//! 多线程 workload runner 和对比表生成。
//!
//! 提供 [`run_all_comparisons`] 一键运行所有场景 × 所有原语组合，
//! 输出格式化对比表。

use crate::metrics::{format_comparison_table, ComparisonRow};
use crate::scenarios::*;
use std::time::Duration;

/// 运行所有对比实验并返回格式化结果。
pub fn run_all_comparisons() -> String {
    let mut output = String::new();
    let mut rows: Vec<ComparisonRow> = Vec::new();

    // ─── 1. Producer-Consumer ───
    output.push_str("=== Producer-Consumer ===\n\n");

    let pc = producer_consumer(8, 4, 4, 200);
    output.push_str(&format!(
        "  Mutex+Sem: produced={}, consumed={}, elapsed={}us\n",
        pc.produced, pc.consumed, pc.elapsed_us
    ));
    rows.push(ComparisonRow {
        name: "Mutex+Sem".into(),
        scenario: "ProducerConsumer".into(),
        metrics: pc.metrics,
    });

    output.push('\n');

    // ─── 2. Readers-Writers (RwLock vs MutexBlocking) ───
    output.push_str("=== Readers-Writers ===\n\n");

    let rw_rwlock = readers_writers(6, 2, 200, 5);
    output.push_str(&format!(
        "  RwLock:  reads={}, writes={}, elapsed={}us\n",
        rw_rwlock.total_reads, rw_rwlock.total_writes, rw_rwlock.elapsed_us
    ));
    rows.push(ComparisonRow {
        name: "RwLock".into(),
        scenario: "ReadersWriters".into(),
        metrics: rw_rwlock.metrics,
    });

    let rw_mutex = readers_writers_mutex(6, 2, 200, 5);
    output.push_str(&format!(
        "  Mutex:   reads={}, writes={}, elapsed={}us\n",
        rw_mutex.total_reads, rw_mutex.total_writes, rw_mutex.elapsed_us
    ));
    rows.push(ComparisonRow {
        name: "MutexBlocking".into(),
        scenario: "ReadersWriters".into(),
        metrics: rw_mutex.metrics,
    });

    output.push('\n');

    // ─── 3. Dining Philosophers ───
    output.push_str("=== Dining Philosophers ===\n\n");

    let dp_ordered = dining_philosophers(5, 100, Duration::from_secs(5), true);
    output.push_str(&format!(
        "  Ordered:   meals={}, deadlock={}, elapsed={}us\n",
        dp_ordered.total_meals, dp_ordered.deadlock_detected, dp_ordered.elapsed_us
    ));
    rows.push(ComparisonRow {
        name: "Ordered".into(),
        scenario: "DiningPhilosophers".into(),
        metrics: dp_ordered.metrics,
    });

    let dp_unordered = dining_philosophers(5, 50, Duration::from_secs(3), false);
    output.push_str(&format!(
        "  Unordered: meals={}, deadlock={}, elapsed={}us\n",
        dp_unordered.total_meals, dp_unordered.deadlock_detected, dp_unordered.elapsed_us
    ));
    rows.push(ComparisonRow {
        name: "Unordered".into(),
        scenario: "DiningPhilosophers".into(),
        metrics: dp_unordered.metrics,
    });

    output.push('\n');

    // ─── Summary table ───
    output.push_str("=== Summary Comparison Table ===\n\n");
    output.push_str(&format_comparison_table(&rows));

    output
}

/// 运行轻量级快速对比（用于 CI / 快速验证）。
pub fn run_quick_comparison() -> String {
    let mut output = String::new();
    let mut rows: Vec<ComparisonRow> = Vec::new();

    let pc = producer_consumer(4, 2, 2, 50);
    rows.push(ComparisonRow {
        name: "Mutex+Sem".into(),
        scenario: "ProdCons(quick)".into(),
        metrics: pc.metrics,
    });

    let rw = readers_writers(3, 1, 50, 2);
    rows.push(ComparisonRow {
        name: "RwLock".into(),
        scenario: "RW(quick)".into(),
        metrics: rw.metrics,
    });

    let dp = dining_philosophers(5, 20, Duration::from_secs(2), true);
    rows.push(ComparisonRow {
        name: "Ordered".into(),
        scenario: "DinPhil(quick)".into(),
        metrics: dp.metrics,
    });

    output.push_str("=== Quick Comparison ===\n\n");
    output.push_str(&format_comparison_table(&rows));
    output
}
