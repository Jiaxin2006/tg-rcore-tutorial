//! 实验 5：同步互斥机制
//!
//! 提供 5 种同步原语（SpinLock / MutexBlocking / Semaphore / Condvar / RwLock），
//! 配合指标采集和经典并发场景进行量化对比。
//!
//! ## 与内核的关系
//!
//! 所有原语的接口与 `tg-rcore-tutorial-sync` 的 `Mutex` trait 完全一致，
//! 可直接替换 ch8 内核中的 `MutexBlocking` 实现。
//! `SpinLock` 和 `RwLock` 是原框架没有的扩展。
//!
//! ## 运行实验
//!
//! ```bash
//! cargo test --test sync_compare -- --nocapture --test-threads=1
//! ```

pub mod metrics;
pub mod primitives;
pub mod scenarios;
pub mod workload;

pub use metrics::{format_comparison_table, ComparisonRow, InstrumentedMutex, SyncMetrics};
pub use primitives::{
    Condvar, Mutex, MutexBlocking, RwLock, Semaphore, SpinLock, ThreadId,
};
