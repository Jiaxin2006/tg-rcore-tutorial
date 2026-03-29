//! 实验 5：同步互斥机制
//!
//! 提供 5 种同步原语（SpinLock / MutexBlocking / Semaphore / Condvar / RwLock），
//! 并同时支持两种使用方式：
//!
//! - `std`：宿主机多线程实验、指标采集、场景对比
//! - `kernel`：`no_std + alloc` 内核可移植实现，可直接替代 `tg_sync`
//!
//! ## 运行实验
//!
//! ```bash
//! cargo test --test sync_compare -- --nocapture --test-threads=1
//! ```

#![cfg_attr(feature = "kernel", no_std)]

#[cfg(all(feature = "std", feature = "kernel"))]
compile_error!("features `std` and `kernel` are mutually exclusive");
#[cfg(not(any(feature = "std", feature = "kernel")))]
compile_error!("enable either `std` (default) or `kernel`");

extern crate alloc;

#[cfg(feature = "std")]
pub mod metrics;
pub mod primitives;
#[cfg(feature = "std")]
pub mod scenarios;
#[cfg(feature = "kernel")]
pub mod up;
#[cfg(feature = "std")]
pub mod workload;

#[cfg(feature = "std")]
pub use metrics::{format_comparison_table, ComparisonRow, InstrumentedMutex, SyncMetrics};
pub use primitives::{
    Condvar, Mutex, MutexBlocking, RwLock, Semaphore, SpinLock, ThreadId,
};
#[cfg(feature = "kernel")]
pub use up::{UPIntrFreeCell, UPIntrRefMut};
