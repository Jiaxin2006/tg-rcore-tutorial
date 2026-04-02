//! 处理器与调度模块
//!
//! ## 与第七章的区别
//!
//! 第七章使用 `PManager`（进程管理器）作为全局处理器类型；
//! 第八章使用 `PThreadManager`（进程 + 线程双层管理器），
//! 支持一个进程拥有多个线程。
//!
//! ## 核心类型
//!
//! - `ProcessorInner = PThreadManager<Process, Thread, ThreadManager, ProcManager>`
//! - `ThreadManager`：由 `exp4-scheduler` 提供的兼容管理器，默认 FIFO
//! - `ProcManager`：管理进程实体
//!
//! 教程阅读建议：
//!
//! - 先看 `ProcessorInner` 类型别名：先建立“统一入口，双层实体”的心智模型；
//! - 再看 `ThreadManager` 与 `ProcManager` 的分工：线程侧由兼容管理器负责 ready queue；
//! - 最后看 `Schedule<ThreadId>`：明确调度粒度已经从进程切换为线程。

use crate::process::{Process, Thread};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::cell::UnsafeCell;
use exp4_scheduler::DefaultTaskManager;
use tg_task_manage::{Manage, PThreadManager, ProcId, ThreadId};

/// 处理器内部类型（双层管理器）
pub type ProcessorInner = PThreadManager<Process, Box<Thread>, ThreadManager, ProcManager>;
/// 默认线程管理器：底层由 `exp4-scheduler` 的 FCFS 兼容管理器提供。
pub type ThreadManager = DefaultTaskManager<Box<Thread>, ThreadId>;

/// 全局处理器包装（通过 `UnsafeCell` 允许内部可变）
pub struct Processor {
    inner: UnsafeCell<ProcessorInner>,
}

unsafe impl Sync for Processor {}

impl Processor {
    /// 创建新处理器
    pub const fn new() -> Self {
        Self { inner: UnsafeCell::new(PThreadManager::new()) }
    }

    /// 获取内部可变引用
    #[inline]
    pub fn get_mut(&self) -> &mut ProcessorInner {
        unsafe { &mut (*self.inner.get()) }
    }
}

/// 全局处理器实例
pub static PROCESSOR: Processor = Processor::new();

/// 进程管理器
///
/// 维护所有进程实体（PID → Process）。
pub struct ProcManager {
    procs: BTreeMap<ProcId, Process>,
}

impl ProcManager {
    /// 创建空的进程管理器
    pub fn new() -> Self {
        Self { procs: BTreeMap::new() }
    }
}

impl Manage<Process, ProcId> for ProcManager {
    /// 插入进程实体
    #[inline]
    fn insert(&mut self, id: ProcId, item: Process) { self.procs.insert(id, item); }
    /// 获取进程可变引用
    #[inline]
    fn get_mut(&mut self, id: ProcId) -> Option<&mut Process> { self.procs.get_mut(&id) }
    /// 删除进程实体
    #[inline]
    fn delete(&mut self, id: ProcId) { self.procs.remove(&id); }
}
