use alloc::collections::BTreeMap;

use crate::scheduler::{DefaultScheduler, PluggableScheduler, Tick};

/// 带任务表的兼容调度管理器。
///
/// 它把“任务对象存储”和“ready queue 策略”组合在一起：
/// - 默认策略是 FCFS，因此行为与旧版 `VecDeque push_back/pop_front` 一致
/// - 若调用方愿意，也可以替换成 RR / MLFQ / CFS-like 等策略
pub struct CompatTaskManager<T, I, S = DefaultScheduler<I>>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    tasks: BTreeMap<I, T>,
    scheduler: S,
    now: Tick,
}

/// 默认兼容管理器，默认使用 FCFS。
pub type DefaultTaskManager<T, I> = CompatTaskManager<T, I, DefaultScheduler<I>>;

impl<T, I> DefaultTaskManager<T, I>
where
    I: Copy + Ord,
{
    /// 创建一个默认 FIFO 兼容管理器。
    pub fn new() -> Self {
        Self::with_scheduler(DefaultScheduler::default())
    }
}

impl<T, I> Default for DefaultTaskManager<T, I>
where
    I: Copy + Ord,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I, S> CompatTaskManager<T, I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    /// 使用指定策略创建兼容管理器。
    pub fn with_scheduler(scheduler: S) -> Self {
        Self {
            tasks: BTreeMap::new(),
            scheduler,
            now: 0,
        }
    }

    /// 设置逻辑时钟。
    pub fn set_now(&mut self, now: Tick) {
        self.now = now;
    }

    /// 推进逻辑时钟。
    pub fn advance_ticks(&mut self, delta: Tick) {
        self.now = self.now.saturating_add(delta);
    }

    /// 当前逻辑时钟。
    pub fn now(&self) -> Tick {
        self.now
    }

    /// 只读访问策略对象。
    pub fn scheduler(&self) -> &S {
        &self.scheduler
    }

    /// 可变访问策略对象。
    pub fn scheduler_mut(&mut self) -> &mut S {
        &mut self.scheduler
    }
}

impl<T, I, S> tg_task_manage::Manage<T, I> for CompatTaskManager<T, I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    fn insert(&mut self, id: I, item: T) {
        self.tasks.insert(id, item);
    }

    fn delete(&mut self, id: I) {
        self.tasks.remove(&id);
    }

    fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.tasks.get_mut(&id)
    }
}

impl<T, I, S> tg_task_manage::Schedule<I> for CompatTaskManager<T, I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    fn add(&mut self, id: I) {
        self.scheduler.enqueue(id, self.now);
    }

    fn fetch(&mut self) -> Option<I> {
        self.scheduler.pick_next(self.now)
    }
}

#[cfg(test)]
mod tests {
    use super::DefaultTaskManager;
    use tg_task_manage::{Manage, Schedule};

    #[test]
    fn default_manager_preserves_fifo_behavior() {
        let mut mgr = DefaultTaskManager::<&'static str, usize>::new();
        mgr.insert(1, "a");
        mgr.insert(2, "b");
        mgr.add(1);
        mgr.add(2);

        assert_eq!(mgr.fetch(), Some(1));
        assert_eq!(mgr.fetch(), Some(2));
        assert_eq!(mgr.get_mut(1), Some(&mut "a"));
    }
}
