//! 实验 4：可插拔调度算法实验套件
//!
//! 包含 FCFS / SJF / RR / MLFQ / CFS-like 五种调度器的完整实现，
//! 以及统一数据采集器和模拟 workload 引擎。
//!
//! 运行实验：`cargo test --test sched_compare -- --nocapture`

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(feature = "task-manage-compat")]
pub mod compat;
pub mod kernel;
pub mod scheduler;
pub mod workload;

#[cfg(feature = "task-manage-compat")]
pub use compat::{CompatTaskManager, DefaultTaskManager};
pub use kernel::KernelSchedulerRuntime;
pub use scheduler::{
    CfsLikeScheduler, DefaultScheduler, EventKind, ExperimentSummary, FcfsScheduler, MetricsCollector,
    MlfqScheduler, PluggableScheduler, RrScheduler, SchedDecision, SchedEvent, Schedule,
    SjfScheduler, TaskStats, Tick,
};
#[cfg(feature = "task-manage-compat")]
pub use tg_task_manage::{Manage, ProcId, ThreadId};
