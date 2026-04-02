use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::ThreadId;

use super::id::ProcId;
use super::manager::Manage;
use super::scheduler::Schedule;
use super::ProcThreadRel;
use core::marker::PhantomData;

const MAX_HARTS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThreadRunState {
    Runnable,
    Running(usize),
    Blocked,
}

#[cfg(feature = "thread")]
/// PThreadManager 数据结构，只管理进程以及进程之间的父子关系
/// P 表示进程, T 表示线程
pub struct PThreadManager<P, T, MT: Manage<T, ThreadId> + Schedule<ThreadId>, MP: Manage<P, ProcId>>
{
    // 进程之间父子关系
    rel_map: BTreeMap<ProcId, ProcThreadRel>,
    // 进程管理
    proc_manager: Option<MP>,
    // 线程所属的进程之间的映射关系
    tid2pid: BTreeMap<ThreadId, ProcId>,
    // 线程在共享调度器中的运行状态
    states: BTreeMap<ThreadId, ThreadRunState>,
    // 进程对象管理和调度
    manager: Option<MT>,
    // 每个 hart 当前正在运行的线程 ID
    current: [Option<ThreadId>; MAX_HARTS],
    phantom_t: PhantomData<T>,
    phantom_p: PhantomData<P>,
}

impl<P, T, MT: Manage<T, ThreadId> + Schedule<ThreadId>, MP: Manage<P, ProcId>>
    PThreadManager<P, T, MT, MP>
{
    /// 新建 PThreadManager
    pub const fn new() -> Self {
        Self {
            rel_map: BTreeMap::new(),
            proc_manager: None,
            tid2pid: BTreeMap::new(),
            states: BTreeMap::new(),
            manager: None,
            current: [None; MAX_HARTS],
            phantom_t: PhantomData::<T>,
            phantom_p: PhantomData::<P>,
        }
    }
    #[inline]
    fn current_slot(&self, hart_id: usize) -> Option<ThreadId> {
        assert!(hart_id < MAX_HARTS, "hart_id {hart_id} exceeds MAX_HARTS={MAX_HARTS}");
        self.current[hart_id]
    }
    #[inline]
    fn current_slot_mut(&mut self, hart_id: usize) -> &mut Option<ThreadId> {
        assert!(hart_id < MAX_HARTS, "hart_id {hart_id} exceeds MAX_HARTS={MAX_HARTS}");
        &mut self.current[hart_id]
    }
    #[inline]
    fn running_hart_for(&self, id: ThreadId, ignore_hart: usize) -> Option<usize> {
        self.current
            .iter()
            .enumerate()
            .find_map(|(hart, slot)| (hart != ignore_hart && *slot == Some(id)).then_some(hart))
    }
    /// 选择下一个线程，只记录 current，不把线程实体借用暴露到外层。
    pub fn find_next_id_for(&mut self, hart_id: usize) -> Option<ThreadId> {
        while let Some(id) = self.manager.as_mut().unwrap().fetch() {
            if let Some(other_hart) = self.running_hart_for(id, hart_id) {
                // 防御性修正：若 ready queue 里残留了“实际上已在其他 hart 运行”的线程，
                // 不再重复调度它，并把状态纠正回 Running(other_hart)。
                self.states.insert(id, ThreadRunState::Running(other_hart));
                continue;
            }
            if self.manager.as_mut().unwrap().get_mut(id).is_some()
                && matches!(self.states.get(&id), Some(ThreadRunState::Runnable))
            {
                self.states.insert(id, ThreadRunState::Running(hart_id));
                *self.current_slot_mut(hart_id) = Some(id);
                return Some(id);
            }
        }
        None
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn find_next_id(&mut self) -> Option<ThreadId> {
        self.find_next_id_for(0)
    }
    /// 找到下一个进程
    pub fn find_next_for(&mut self, hart_id: usize) -> Option<&mut T> {
        let id = self.find_next_id_for(hart_id)?;
        self.manager.as_mut().unwrap().get_mut(id)
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn find_next(&mut self) -> Option<&mut T> {
        self.find_next_for(0)
    }
    /// 设置 manager
    pub fn set_manager(&mut self, manager: MT) {
        self.manager = Some(manager);
    }
    /// 设置 proc_manager
    pub fn set_proc_manager(&mut self, proc_manager: MP) {
        self.proc_manager = Some(proc_manager);
    }
    /// 当前线程重新入队
    pub fn make_current_suspend_for(&mut self, hart_id: usize) {
        if let Some(id) = self.current_slot(hart_id) {
            if let Some(other_hart) = self.running_hart_for(id, hart_id) {
                self.states.insert(id, ThreadRunState::Running(other_hart));
            } else {
                self.states.insert(id, ThreadRunState::Runnable);
                self.manager.as_mut().unwrap().add(id);
            }
            *self.current_slot_mut(hart_id) = None;
        }
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn make_current_suspend(&mut self) {
        self.make_current_suspend_for(0);
    }
    /// 结束当前线程
    pub fn make_current_exited_for(&mut self, hart_id: usize, exit_code: isize) {
        if let Some(id) = self.current_slot(hart_id) {
            self.states.remove(&id);
            self.manager.as_mut().unwrap().delete(id);
            // 线程结束时维护与父进程之间的关系
            let pid = self.tid2pid.remove(&id).unwrap();
            let mut flag = false;
            if let Some(current_rel) = self.rel_map.get_mut(&pid) {
                current_rel.del_thread(id, exit_code);
                // 如果线程数量为 0，则需要把当前线程所属的进程给删除掉（所有等待的线程都已经结束）
                if current_rel.threads.is_empty() {
                    flag = true;
                }
            }
            if flag {
                // 教学语义：最后一个线程退出时，进程对象也随之清理。
                self.del_proc(pid, exit_code);
            }
            *self.current_slot_mut(hart_id) = None;
        }
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn make_current_exited(&mut self, exit_code: isize) {
        self.make_current_exited_for(0, exit_code);
    }
    /// 让当前线程阻塞
    pub fn make_current_blocked_for(&mut self, hart_id: usize) {
        if self.current_slot(hart_id).is_some() {
            if let Some(id) = self.current_slot(hart_id) {
                self.states.insert(id, ThreadRunState::Blocked);
            }
            *self.current_slot_mut(hart_id) = None;
        }
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn make_current_blocked(&mut self) {
        self.make_current_blocked_for(0);
    }
    /// 某个线程重新入队
    pub fn re_enque(&mut self, id: ThreadId) {
        if matches!(self.states.get(&id), Some(ThreadRunState::Blocked)) {
            self.states.insert(id, ThreadRunState::Runnable);
            self.manager.as_mut().unwrap().add(id);
        }
    }
    /// 添加线程
    pub fn add(&mut self, id: ThreadId, task: T, pid: ProcId) {
        self.manager.as_mut().unwrap().insert(id, task);
        self.states.insert(id, ThreadRunState::Runnable);
        self.manager.as_mut().unwrap().add(id);
        // 增加线程与进程之间的从属关系
        if let Some(parent_rel) = self.rel_map.get_mut(&pid) {
            parent_rel.add_thread(id);
            self.tid2pid.insert(id, pid);
        }
    }
    /// 当前线程
    pub fn current_for(&mut self, hart_id: usize) -> Option<&mut T> {
        let id = self.current_slot(hart_id)?;
        self.manager.as_mut().unwrap().get_mut(id)
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn current(&mut self) -> Option<&mut T> {
        self.current_for(0)
    }
    /// 当前线程 ID。
    #[inline]
    pub fn current_tid_for(&self, hart_id: usize) -> Option<ThreadId> {
        self.current_slot(hart_id)
    }
    /// 是否存在某个 hart 正在运行线程。
    #[inline]
    pub fn has_running_threads(&self) -> bool {
        self.current.iter().any(Option::is_some)
    }
    /// 除指定 hart 外，是否还有其他 hart 正在运行线程。
    #[inline]
    pub fn has_running_threads_other_than(&self, hart_id: usize) -> bool {
        self.current
            .iter()
            .enumerate()
            .any(|(idx, tid)| idx != hart_id && tid.is_some())
    }
    /// 兼容旧接口：默认使用 hart0。
    #[inline]
    pub fn current_tid(&self) -> Option<ThreadId> {
        self.current_tid_for(0)
    }
    /// 当前线程所属进程 ID。
    #[inline]
    pub fn current_pid_for(&self, hart_id: usize) -> Option<ProcId> {
        self.current_slot(hart_id)
            .and_then(|tid| self.tid2pid.get(&tid).copied())
    }
    /// 兼容旧接口：默认使用 hart0。
    #[inline]
    pub fn current_pid(&self) -> Option<ProcId> {
        self.current_pid_for(0)
    }
    /// 获取某个线程
    #[inline]
    pub fn get_task(&mut self, id: ThreadId) -> Option<&mut T> {
        self.manager.as_mut().unwrap().get_mut(id)
    }

    /// 在一个短作用域内借用当前线程。
    #[inline]
    pub fn with_current_task_for<R>(
        &mut self,
        hart_id: usize,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let tid = self.current_slot(hart_id)?;
        let task = self.manager.as_mut().unwrap().get_mut(tid)? as *mut T;
        Some(unsafe { f(&mut *task) })
    }
    /// 兼容旧接口：默认使用 hart0。
    #[inline]
    pub fn with_current_task<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.with_current_task_for(0, f)
    }

    /// 在一个短作用域内借用当前进程。
    #[inline]
    pub fn with_current_proc_for<R>(
        &mut self,
        hart_id: usize,
        f: impl FnOnce(&mut P) -> R,
    ) -> Option<R> {
        let tid = self.current_slot(hart_id)?;
        let pid = *self.tid2pid.get(&tid)?;
        let proc = self.proc_manager.as_mut().unwrap().get_mut(pid)? as *mut P;
        Some(unsafe { f(&mut *proc) })
    }
    /// 兼容旧接口：默认使用 hart0。
    #[inline]
    pub fn with_current_proc<R>(&mut self, f: impl FnOnce(&mut P) -> R) -> Option<R> {
        self.with_current_proc_for(0, f)
    }

    /// 在一个短作用域内同时借用当前线程和所属进程。
    ///
    /// 线程对象与进程对象来自两个独立的管理器，因此这里将借用限制在闭包内，
    /// 避免把两个 `&mut` 引用泄露到外层并跨越调度器状态更新。
    #[inline]
    pub fn with_current_task_proc_for<R>(
        &mut self,
        hart_id: usize,
        f: impl FnOnce(&mut T, &mut P) -> R,
    ) -> Option<R> {
        let tid = self.current_slot(hart_id)?;
        let pid = *self.tid2pid.get(&tid)?;
        let task = self.manager.as_mut().unwrap().get_mut(tid)? as *mut T;
        let proc = self.proc_manager.as_mut().unwrap().get_mut(pid)? as *mut P;
        Some(unsafe { f(&mut *task, &mut *proc) })
    }
    /// 兼容旧接口：默认使用 hart0。
    #[inline]
    pub fn with_current_task_proc<R>(
        &mut self,
        f: impl FnOnce(&mut T, &mut P) -> R,
    ) -> Option<R> {
        self.with_current_task_proc_for(0, f)
    }
    /// 添加进程
    pub fn add_proc(&mut self, id: ProcId, proc: P, parent: ProcId) {
        self.proc_manager.as_mut().unwrap().insert(id, proc);
        if let Some(parent_rel) = self.rel_map.get_mut(&parent) {
            parent_rel.add_child(id);
        }
        self.rel_map.insert(id, ProcThreadRel::new(parent));
    }
    /// 查询进程
    pub fn get_proc(&mut self, id: ProcId) -> Option<&mut P> {
        self.proc_manager.as_mut().unwrap().get_mut(id)
    }
    /// 结束当前进程
    pub fn del_proc(&mut self, id: ProcId, exit_code: isize) {
        // 删除进程实体
        self.proc_manager.as_mut().unwrap().delete(id);
        // 进程结束时维护父子关系，进程删除后，所有的子进程交给 0 号进程来维护
        let current_rel = self.rel_map.remove(&id).unwrap();
        let parent_pid = current_rel.parent;
        let children = current_rel.children;
        let init_pid = ProcId::from_usize(0);
        // 从父进程中删除当前进程
        if let Some(parent_rel) = self.rel_map.get_mut(&parent_pid) {
            parent_rel.del_child(id, exit_code);
        }
        // 把当前进程的所有子进程转移到 0 号进程。
        // 如果当前删除的正是 0 号进程，或 0 号进程已经不存在，则保留原父进程，
        // 避免在“孤儿收养者”不存在时再次触发 panic。
        let orphan_parent = if id != init_pid && self.rel_map.contains_key(&init_pid) {
            init_pid
        } else {
            parent_pid
        };
        for i in children {
            if let Some(child_rel) = self.rel_map.get_mut(&i) {
                child_rel.parent = orphan_parent;
            }
            if let Some(orphan_rel) = self.rel_map.get_mut(&orphan_parent) {
                orphan_rel.add_child(i);
            }
        }
    }
    /// wait 系统调用，返回结束的子进程 id 和 exit_code，正在运行的子进程不返回 None，返回 (-2, -1)
    pub fn wait_for(&mut self, hart_id: usize, child_pid: ProcId) -> Option<(ProcId, isize)> {
        let id = self.current_slot(hart_id)?;
        let pid = self.tid2pid.get(&id).unwrap();
        let current_rel = self.rel_map.get_mut(pid).unwrap();
        if child_pid.get_usize() == usize::MAX {
            current_rel.wait_any_child()
        } else {
            current_rel.wait_child(child_pid)
        }
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn wait(&mut self, child_pid: ProcId) -> Option<(ProcId, isize)> {
        self.wait_for(0, child_pid)
    }
    /// wait_tid 系统调用
    pub fn waittid_for(&mut self, hart_id: usize, thread_tid: ThreadId) -> Option<isize> {
        // 返回值约定：-2 表示目标线程还活着，需要继续等待。
        let id = self.current_slot(hart_id)?;
        let pid = self.tid2pid.get(&id).unwrap();
        let current_rel = self.rel_map.get_mut(pid).unwrap();
        current_rel.wait_thread(thread_tid)
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn waittid(&mut self, thread_tid: ThreadId) -> Option<isize> {
        self.waittid_for(0, thread_tid)
    }
    /// 某个进程的线程数量
    pub fn thread_count(&self, id: ProcId) -> usize {
        self.rel_map.get(&id).unwrap().threads.len()
    }
    /// 查询进程的线程
    pub fn get_thread(&mut self, id: ProcId) -> Option<&Vec<ThreadId>> {
        self.rel_map.get_mut(&id).map(|p| &p.threads)
    }
    /// 获取当前线程所属的进程
    pub fn get_current_proc_for(&mut self, hart_id: usize) -> Option<&mut P> {
        if let Some(id) = self.current_slot(hart_id) {
            let pid = self.tid2pid.get(&id).unwrap();
            self.proc_manager.as_mut().unwrap().get_mut(*pid)
        } else {
            None
        }
    }
    /// 兼容旧接口：默认使用 hart0。
    pub fn get_current_proc(&mut self) -> Option<&mut P> {
        self.get_current_proc_for(0)
    }
}
