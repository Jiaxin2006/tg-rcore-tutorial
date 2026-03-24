//! 同步原语指标采集
//!
//! 提供 [`SyncMetrics`] 统计结构和 [`InstrumentedMutex`] 装饰器，
//! 可自动记录锁竞争、持锁时间、等待时间、上下文切换和 starvation。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use crate::primitives::{Mutex, ThreadId};

/// 单次锁操作的记录。
#[derive(Clone, Debug)]
pub struct LockEvent {
    /// 操作线程。
    pub tid: ThreadId,
    /// 事件类型。
    pub kind: LockEventKind,
    /// 相对于实验开始的时间（微秒）。
    pub timestamp_us: u64,
}

/// 锁事件类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockEventKind {
    /// 尝试获取锁。
    TryAcquire,
    /// 成功获取锁（无竞争）。
    Acquired,
    /// 获取失败（竞争，需等待）。
    Contended,
    /// 等待后成功获取。
    AcquiredAfterWait,
    /// 释放锁。
    Released,
}

/// 同步原语的汇总指标。
#[derive(Clone, Debug, Default)]
pub struct SyncMetrics {
    /// 锁竞争次数（`lock` 返回 `false`）。
    pub contention_count: u64,
    /// 成功获取锁的总次数。
    pub acquire_count: u64,
    /// 总持锁时间（微秒）。
    pub total_hold_us: u64,
    /// 总等待时间（微秒）。
    pub total_wait_us: u64,
    /// 最大单次等待时间（微秒）。
    pub max_wait_us: u64,
    /// 上下文切换次数（sleep lock 中线程 park 的次数）。
    pub ctx_switch_count: u64,
    /// 等待超过阈值的次数。
    pub starvation_events: u64,
    /// 每线程等待时间（用于公平性分析）。
    pub per_thread_wait_us: HashMap<ThreadId, u64>,
    /// 每线程获取锁次数。
    pub per_thread_acquire_count: HashMap<ThreadId, u64>,
}

impl SyncMetrics {
    /// 平均持锁时间（微秒）。
    pub fn avg_hold_us(&self) -> f64 {
        if self.acquire_count == 0 {
            0.0
        } else {
            self.total_hold_us as f64 / self.acquire_count as f64
        }
    }

    /// 平均等待时间（微秒）。
    pub fn avg_wait_us(&self) -> f64 {
        let waits = self.contention_count;
        if waits == 0 {
            0.0
        } else {
            self.total_wait_us as f64 / waits as f64
        }
    }

    /// 公平性指标：等待时间的变异系数（标准差 / 均值）。
    /// 值越小越公平，0 表示完全公平。
    pub fn fairness_cv(&self) -> f64 {
        let waits: Vec<f64> = self.per_thread_wait_us.values().map(|&v| v as f64).collect();
        if waits.is_empty() {
            return 0.0;
        }
        let mean = waits.iter().sum::<f64>() / waits.len() as f64;
        if mean == 0.0 {
            return 0.0;
        }
        let variance = waits.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / waits.len() as f64;
        variance.sqrt() / mean
    }

    /// 合并另一个 SyncMetrics（用于汇总多线程结果）。
    pub fn merge(&mut self, other: &SyncMetrics) {
        self.contention_count += other.contention_count;
        self.acquire_count += other.acquire_count;
        self.total_hold_us += other.total_hold_us;
        self.total_wait_us += other.total_wait_us;
        self.max_wait_us = self.max_wait_us.max(other.max_wait_us);
        self.ctx_switch_count += other.ctx_switch_count;
        self.starvation_events += other.starvation_events;
        for (&tid, &wait) in &other.per_thread_wait_us {
            *self.per_thread_wait_us.entry(tid).or_insert(0) += wait;
        }
        for (&tid, &count) in &other.per_thread_acquire_count {
            *self.per_thread_acquire_count.entry(tid).or_insert(0) += count;
        }
    }
}

impl std::fmt::Display for SyncMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "  contention:      {}", self.contention_count)?;
        writeln!(f, "  acquires:        {}", self.acquire_count)?;
        writeln!(f, "  avg hold (us):   {:.1}", self.avg_hold_us())?;
        writeln!(f, "  avg wait (us):   {:.1}", self.avg_wait_us())?;
        writeln!(f, "  max wait (us):   {}", self.max_wait_us)?;
        writeln!(f, "  ctx switches:    {}", self.ctx_switch_count)?;
        writeln!(f, "  starvation:      {}", self.starvation_events)?;
        writeln!(f, "  fairness CV:     {:.3}", self.fairness_cv())
    }
}

// ---------------------------------------------------------------------------
// InstrumentedMutex — 装饰器
// ---------------------------------------------------------------------------

/// Starvation 判定阈值（微秒）：等待超过此值视为 starvation。
const STARVATION_THRESHOLD_US: u64 = 10_000;

/// 为任意 `Mutex` 实现自动采集指标的装饰器。
///
/// 在 `lock` / `unlock` 路径中记录时间戳，汇总到 `SyncMetrics`。
pub struct InstrumentedMutex<M: Mutex> {
    inner: M,
    contention: AtomicU64,
    acquires: AtomicU64,
    ctx_switches: AtomicU64,
    starvation: AtomicU64,
    detail: StdMutex<InstrumentedDetail>,
}

struct InstrumentedDetail {
    total_hold_us: u64,
    total_wait_us: u64,
    max_wait_us: u64,
    per_thread_wait_us: HashMap<ThreadId, u64>,
    per_thread_acquire_count: HashMap<ThreadId, u64>,
    acquire_timestamps: HashMap<ThreadId, Instant>,
}

impl<M: Mutex> InstrumentedMutex<M> {
    /// 创建带指标采集的包装。
    pub fn new(inner: M) -> Self {
        Self {
            inner,
            contention: AtomicU64::new(0),
            acquires: AtomicU64::new(0),
            ctx_switches: AtomicU64::new(0),
            starvation: AtomicU64::new(0),
            detail: StdMutex::new(InstrumentedDetail {
                total_hold_us: 0,
                total_wait_us: 0,
                max_wait_us: 0,
                per_thread_wait_us: HashMap::new(),
                per_thread_acquire_count: HashMap::new(),
                acquire_timestamps: HashMap::new(),
            }),
        }
    }

    /// 获取底层锁的引用。
    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// 记录一次成功获取。
    fn record_acquire(&self, tid: ThreadId) {
        self.acquires.fetch_add(1, Ordering::Relaxed);
        let mut d = self.detail.lock().unwrap();
        *d.per_thread_acquire_count.entry(tid).or_insert(0) += 1;
        d.acquire_timestamps.insert(tid, Instant::now());
    }

    /// 记录一次释放（计算持锁时间）。
    fn record_release(&self, tid: ThreadId) {
        let mut d = self.detail.lock().unwrap();
        if let Some(acq_time) = d.acquire_timestamps.remove(&tid) {
            let hold_us = acq_time.elapsed().as_micros() as u64;
            d.total_hold_us += hold_us;
        }
    }

    /// 记录等待时间。
    fn record_wait(&self, tid: ThreadId, wait_us: u64) {
        self.contention.fetch_add(1, Ordering::Relaxed);
        self.ctx_switches.fetch_add(1, Ordering::Relaxed);
        if wait_us > STARVATION_THRESHOLD_US {
            self.starvation.fetch_add(1, Ordering::Relaxed);
        }
        let mut d = self.detail.lock().unwrap();
        d.total_wait_us += wait_us;
        d.max_wait_us = d.max_wait_us.max(wait_us);
        *d.per_thread_wait_us.entry(tid).or_insert(0) += wait_us;
    }

    /// 导出当前汇总指标。
    pub fn metrics(&self) -> SyncMetrics {
        let d = self.detail.lock().unwrap();
        SyncMetrics {
            contention_count: self.contention.load(Ordering::Relaxed),
            acquire_count: self.acquires.load(Ordering::Relaxed),
            total_hold_us: d.total_hold_us,
            total_wait_us: d.total_wait_us,
            max_wait_us: d.max_wait_us,
            ctx_switch_count: self.ctx_switches.load(Ordering::Relaxed),
            starvation_events: self.starvation.load(Ordering::Relaxed),
            per_thread_wait_us: d.per_thread_wait_us.clone(),
            per_thread_acquire_count: d.per_thread_acquire_count.clone(),
        }
    }

    /// 使用 `Mutex` trait 接口尝试获取锁。
    pub fn try_lock(&self, tid: ThreadId) -> bool {
        let result = self.inner.lock(tid);
        if result {
            self.record_acquire(tid);
        }
        result
    }

    /// 使用 `Mutex` trait 接口释放锁（返回被唤醒的线程）。
    pub fn try_unlock(&self, holder_tid: ThreadId) -> Option<ThreadId> {
        self.record_release(holder_tid);
        self.inner.unlock()
    }

    /// 阻塞式获取：自旋等待直到获取成功，记录等待时间。
    pub fn blocking_lock(&self, tid: ThreadId) {
        let wait_start = Instant::now();
        if self.inner.lock(tid) {
            self.record_acquire(tid);
            return;
        }
        // 需要等待
        loop {
            std::thread::yield_now();
            if self.inner.lock(tid) {
                let wait_us = wait_start.elapsed().as_micros() as u64;
                self.record_wait(tid, wait_us);
                self.record_acquire(tid);
                return;
            }
        }
    }

    /// 阻塞式释放。
    pub fn blocking_unlock(&self, tid: ThreadId) -> Option<ThreadId> {
        self.record_release(tid);
        self.inner.unlock()
    }
}

impl<M: Mutex> Mutex for InstrumentedMutex<M> {
    fn lock(&self, tid: ThreadId) -> bool {
        let result = self.inner.lock(tid);
        if result {
            self.record_acquire(tid);
        } else {
            self.contention.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    fn unlock(&self) -> Option<ThreadId> {
        // We can't know the holder tid here, so just delegate
        self.inner.unlock()
    }

    fn holder(&self) -> Option<ThreadId> {
        self.inner.holder()
    }

    fn waiting(&self) -> Vec<ThreadId> {
        self.inner.waiting()
    }
}

// ---------------------------------------------------------------------------
// Comparison table formatting
// ---------------------------------------------------------------------------

/// 一行对比结果。
#[derive(Clone, Debug)]
pub struct ComparisonRow {
    /// 原语/策略名称。
    pub name: String,
    /// 场景名称。
    pub scenario: String,
    /// 汇总指标。
    pub metrics: SyncMetrics,
}

/// 输出对比表到字符串。
pub fn format_comparison_table(rows: &[ComparisonRow]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "{:<18} {:<20} {:>10} {:>10} {:>12} {:>12} {:>10} {:>10} {:>10}\n",
        "Primitive", "Scenario", "Contend", "Acquires", "AvgHold(us)", "AvgWait(us)", "MaxWait", "CtxSw", "Starve"
    ));
    s.push_str(&"-".repeat(112));
    s.push('\n');
    for row in rows {
        let m = &row.metrics;
        s.push_str(&format!(
            "{:<18} {:<20} {:>10} {:>10} {:>12.1} {:>12.1} {:>10} {:>10} {:>10}\n",
            row.name,
            row.scenario,
            m.contention_count,
            m.acquire_count,
            m.avg_hold_us(),
            m.avg_wait_us(),
            m.max_wait_us,
            m.ctx_switch_count,
            m.starvation_events,
        ));
    }
    s
}
