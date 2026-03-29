use crate::scheduler::{
    ExperimentSummary, MetricsCollector, PluggableScheduler, SchedDecision, Schedule, Tick,
};

/// 内核调度接入层。
///
/// 该结构把实验中的 `PluggableScheduler` 包装成一个可嵌入内核管理器的运行时：
///
/// - 内核继续自己维护任务/线程实体与上下文切换
/// - `KernelSchedulerRuntime` 只负责 ready 队列策略、当前运行任务 ID、时钟推进与统一埋点
/// - 方法命名刻意贴近 `ch5-ch8` 里现有的调用习惯，方便把原来的
///   `ready_queue.push_back/pop_front` 替换成统一 hook
pub struct KernelSchedulerRuntime<I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    /// 调度策略对象。
    pub policy: S,
    /// 指标采集器。
    pub metrics: MetricsCollector<I>,
    now: Tick,
    current: Option<I>,
    ready_len: usize,
}

impl<I, S> KernelSchedulerRuntime<I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    /// 创建新的内核调度运行时。
    pub fn new(policy: S, starvation_threshold: u64) -> Self {
        Self {
            policy,
            metrics: MetricsCollector::new(starvation_threshold),
            now: 0,
            current: None,
            ready_len: 0,
        }
    }

    /// 当前逻辑时钟。
    pub fn now(&self) -> Tick {
        self.now
    }

    /// 直接设置当前逻辑时钟。
    ///
    /// 适合内核使用真实硬件时钟或 `time::read()` 同步实验时钟。
    pub fn set_now(&mut self, now: Tick) {
        self.now = now;
    }

    /// 推进逻辑时钟。
    pub fn advance_ticks(&mut self, delta: Tick) {
        self.now = self.now.saturating_add(delta);
    }

    /// 当前运行任务。
    pub fn current(&self) -> Option<I> {
        self.current
    }

    /// 当前 ready 队列长度。
    pub fn ready_len(&self) -> usize {
        self.ready_len
    }

    /// 只读访问策略对象。
    pub fn policy(&self) -> &S {
        &self.policy
    }

    /// 可变访问策略对象。
    pub fn policy_mut(&mut self) -> &mut S {
        &mut self.policy
    }

    /// 只读访问指标采集器。
    pub fn metrics(&self) -> &MetricsCollector<I> {
        &self.metrics
    }

    /// 可变访问指标采集器。
    pub fn metrics_mut(&mut self) -> &mut MetricsCollector<I> {
        &mut self.metrics
    }

    /// 任务进入 ready 队列。
    ///
    /// 对应 `spawn/fork/thread_create` 或普通的“新任务到达”路径。
    pub fn add_task(&mut self, id: I) {
        self.metrics.record_enqueue(self.now, id, self.ready_len);
        self.policy.enqueue(id, self.now);
        self.ready_len = self.ready_len.saturating_add(1);
    }

    /// 选择下一个运行任务。
    ///
    /// 对应 `find_next/fetch` 路径。
    pub fn dispatch_next(&mut self) -> Option<I> {
        debug_assert!(
            self.current.is_none(),
            "dispatch_next called while another task is still marked current"
        );
        let next = self.policy.pick_next(self.now)?;
        self.ready_len = self.ready_len.saturating_sub(1);
        self.current = Some(next);
        self.metrics.record_dispatch(self.now, next, self.ready_len);
        Some(next)
    }

    /// 当前任务主动让出 CPU 或被抢占后重新入队。
    ///
    /// 对应 `make_current_suspend`。
    pub fn make_current_suspend(&mut self) -> Option<I> {
        let id = self.current.take()?;
        self.metrics.record_preempt(self.now, id, self.ready_len);
        self.policy.enqueue(id, self.now);
        self.ready_len = self.ready_len.saturating_add(1);
        Some(id)
    }

    /// `make_current_suspend` 的语义别名，更贴近抢占式调度语境。
    pub fn preempt_current(&mut self) -> Option<I> {
        self.make_current_suspend()
    }

    /// 当前任务进入阻塞态。
    ///
    /// 对应 `make_current_blocked`。
    pub fn make_current_blocked(&mut self) -> Option<I> {
        let id = self.current.take()?;
        self.policy.on_block(id, self.now);
        self.metrics.record_block(self.now, id, self.ready_len);
        Some(id)
    }

    /// 阻塞任务被唤醒，重新回到 ready 队列。
    ///
    /// 对应 `re_enque` / wakeup 路径。
    pub fn re_enque(&mut self, id: I) {
        self.metrics.record_wakeup(self.now, id, self.ready_len);
        self.policy.on_wakeup(id, self.now);
        self.ready_len = self.ready_len.saturating_add(1);
    }

    /// 当前任务退出。
    ///
    /// 对应 `make_current_exited`。
    pub fn make_current_exited(&mut self) -> Option<I> {
        let id = self.current.take()?;
        self.policy.on_exit(id, self.now);
        self.metrics.record_exit(self.now, id, self.ready_len);
        Some(id)
    }

    /// 记录一次时钟 tick，并把决策返回给内核。
    ///
    /// 对于 RR / MLFQ / CFS-like，内核应在返回 `Preempt` 时调用
    /// `make_current_suspend` 或自定义的上下文切换逻辑。
    pub fn on_tick(&mut self) -> Option<SchedDecision<I>> {
        let current = self.current?;
        self.metrics.record_tick(self.now, Some(current), self.ready_len);
        Some(self.policy.on_tick(current, self.now))
    }

    /// 汇总当前采集到的实验指标。
    pub fn summary(&self) -> ExperimentSummary {
        self.metrics.summary()
    }
}

impl<I, S> Schedule<I> for KernelSchedulerRuntime<I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    fn add(&mut self, id: I) {
        self.add_task(id);
    }

    fn fetch(&mut self) -> Option<I> {
        self.dispatch_next()
    }
}

#[cfg(test)]
mod tests {
    use super::KernelSchedulerRuntime;
    use crate::scheduler::{EventKind, FcfsScheduler, RrScheduler, Schedule, SchedDecision};

    #[test]
    fn kernel_runtime_matches_ch8_style_lifecycle() {
        let mut rt = KernelSchedulerRuntime::new(FcfsScheduler::<usize>::new(), 10);

        rt.add(1);
        rt.add(2);
        assert_eq!(rt.ready_len(), 2);

        assert_eq!(rt.fetch(), Some(1));
        assert_eq!(rt.current(), Some(1));
        assert_eq!(rt.ready_len(), 1);

        rt.advance_ticks(3);
        assert_eq!(rt.make_current_blocked(), Some(1));
        assert_eq!(rt.current(), None);
        assert_eq!(rt.ready_len(), 1);

        rt.advance_ticks(2);
        rt.re_enque(1);
        assert_eq!(rt.ready_len(), 2);

        assert_eq!(rt.fetch(), Some(2));
        rt.advance_ticks(1);
        assert_eq!(rt.make_current_exited(), Some(2));

        assert_eq!(rt.fetch(), Some(1));
        rt.advance_ticks(4);
        assert_eq!(rt.make_current_exited(), Some(1));

        let summary = rt.summary();
        assert_eq!(summary.starvation_total, 0);
        assert_eq!(rt.metrics.events.len(), 9);
        assert_eq!(rt.metrics.events[0].kind, EventKind::Enqueue);
        assert_eq!(rt.metrics.events[3].kind, EventKind::Block);
        assert_eq!(rt.metrics.events[4].kind, EventKind::Wakeup);

        let task1 = rt.metrics.per_task.get(&1).unwrap();
        let task2 = rt.metrics.per_task.get(&2).unwrap();
        assert_eq!(task1.run_ticks, 7);
        assert_eq!(task2.run_ticks, 1);
        assert_eq!(task1.wakeup_latencies.as_slice(), &[1]);
        assert_eq!(task2.wait_ticks, 5);
    }

    #[test]
    fn rr_policy_can_be_driven_through_kernel_runtime_ticks() {
        let mut rt = KernelSchedulerRuntime::new(RrScheduler::<usize>::new(2), 0);

        rt.add_task(1);
        rt.add_task(2);
        assert_eq!(rt.dispatch_next(), Some(1));

        rt.advance_ticks(1);
        assert_eq!(rt.on_tick(), Some(SchedDecision::KeepRunning));
        assert_eq!(rt.current(), Some(1));

        rt.advance_ticks(1);
        assert_eq!(rt.on_tick(), Some(SchedDecision::Preempt));
        assert_eq!(rt.preempt_current(), Some(1));
        assert_eq!(rt.dispatch_next(), Some(2));

        let kinds: alloc::vec::Vec<_> = rt.metrics.events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds.as_slice(),
            &[
                EventKind::Enqueue,
                EventKind::Enqueue,
                EventKind::Dispatch,
                EventKind::Tick,
                EventKind::Tick,
                EventKind::Preempt,
                EventKind::Dispatch,
            ]
        );
    }
}
