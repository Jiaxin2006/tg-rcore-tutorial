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
//! - `ThreadManager`：按 hart 划分本地 ready queue，并在空队列时从其他 hart 偷取任务
//! - `ProcManager`：管理进程实体
//!
//! 教程阅读建议：
//!
//! - 先看 `ProcessorInner` 类型别名：先建立“统一入口，双层实体”的心智模型；
//! - 再看 `ThreadManager` 与 `ProcManager` 的分工：线程侧负责 work stealing ready queue；
//! - 最后看 `HartSchedule<ThreadId>`：明确调度粒度已经从进程切换为线程。

use crate::process::{Process, Thread};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use core::cell::UnsafeCell;
use tg_task_manage::{HartSchedule, Manage, PThreadManager, ProcId, Schedule, ThreadId};

const MAX_HARTS: usize = tg_syscall::SMP_HART_CAPACITY;

/// 处理器内部类型（双层管理器）
pub type ProcessorInner = PThreadManager<Process, Box<Thread>, ThreadManager, ProcManager>;
/// 默认线程管理器：每个 hart 一条本地队列，空队列时从其他 hart 偷取任务。
pub struct ThreadManager {
    tasks: BTreeMap<ThreadId, Box<Thread>>,
    ready: [VecDeque<ThreadId>; MAX_HARTS],
}

impl ThreadManager {
    /// 创建空的多核线程管理器。
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            ready: core::array::from_fn(|_| VecDeque::new()),
        }
    }

    #[inline]
    fn normalize_hart(hart_id: usize) -> usize {
        hart_id % MAX_HARTS
    }

    #[inline]
    fn queue_for(&mut self, hart_id: usize) -> &mut VecDeque<ThreadId> {
        &mut self.ready[Self::normalize_hart(hart_id)]
    }

    fn steal_for(&mut self, thief_hart: usize) -> Option<ThreadId> {
        let thief_hart = Self::normalize_hart(thief_hart);
        let mut victim = None;
        let mut victim_len = 0usize;
        for offset in 1..MAX_HARTS {
            let hart = (thief_hart + offset) % MAX_HARTS;
            let len = self.ready[hart].len();
            if len > victim_len {
                victim = Some(hart);
                victim_len = len;
            }
        }
        victim.and_then(|hart| self.ready[hart].pop_back())
    }
}

impl Manage<Box<Thread>, ThreadId> for ThreadManager {
    #[inline]
    fn insert(&mut self, id: ThreadId, item: Box<Thread>) {
        self.tasks.insert(id, item);
    }

    #[inline]
    fn delete(&mut self, id: ThreadId) {
        self.tasks.remove(&id);
    }

    #[inline]
    fn get_mut(&mut self, id: ThreadId) -> Option<&mut Box<Thread>> {
        self.tasks.get_mut(&id)
    }
}

impl Schedule<ThreadId> for ThreadManager {
    #[inline]
    fn add(&mut self, id: ThreadId) {
        self.add_for(0, id);
    }

    #[inline]
    fn fetch(&mut self) -> Option<ThreadId> {
        self.fetch_for(0)
    }
}

impl HartSchedule<ThreadId> for ThreadManager {
    #[inline]
    fn add_for(&mut self, hart_id: usize, id: ThreadId) {
        self.queue_for(hart_id).push_back(id);
    }

    #[inline]
    fn fetch_for(&mut self, hart_id: usize) -> Option<ThreadId> {
        let local_hart = Self::normalize_hart(hart_id);
        self.ready[local_hart]
            .pop_front()
            .or_else(|| self.steal_for(local_hart))
    }
}

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
