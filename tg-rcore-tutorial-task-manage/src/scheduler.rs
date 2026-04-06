use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// 全局时间单位（通常是时钟 tick）。
pub type Tick = u64;

/// 兼容旧版本的简化调度接口。
///
/// - `I` 即任务 ID 类型（例如 `ProcId` 或 `ThreadId`）。
/// - 该 trait 只包含「入队/取队」，适合最基础调度器。
pub trait Schedule<I: Copy + Ord> {
    /// 将任务 ID 放入就绪队列。
    fn add(&mut self, id: I);
    /// 从就绪队列取出下一个可运行任务 ID。
    fn fetch(&mut self) -> Option<I>;
}

/// 面向多核的调度接口。
///
/// 与只暴露“全局 add/fetch”的 [`Schedule`] 相比，这个 trait 允许调用方显式指定：
/// - 任务应进入哪个 hart 的本地就绪队列
/// - 当前 hart 取本地任务时，是否允许从其他 hart 偷取任务
pub trait HartSchedule<I: Copy + Ord>: Schedule<I> {
    /// 将任务放入指定 hart 的本地就绪队列。
    fn add_for(&mut self, hart_id: usize, id: I);
    /// 为指定 hart 选择下一个任务。
    ///
    /// 具体实现可以先查本地队列，再按策略从其他 hart 偷取任务。
    fn fetch_for(&mut self, hart_id: usize) -> Option<I>;
}

/// 调度决策结果（供时钟中断等路径使用）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedDecision<I: Copy> {
    /// 当前任务继续运行，不切换。
    KeepRunning,
    /// 当前任务被抢占，外部应触发一次调度切换。
    Preempt,
    /// 直接切换到指定任务（用于高级策略，可选）。
    SwitchTo(I),
}

/// 可插拔调度策略接口（实验建议版本）。
///
/// 相比 `Schedule`，新增了：
/// - `on_tick`：时钟中断驱动抢占/时间片管理
/// - `on_block`：任务阻塞时更新策略内部状态
/// - `on_wakeup`：任务唤醒时重新进入就绪结构
pub trait PluggableScheduler<I: Copy + Ord> {
    /// 任务进入就绪队列。
    fn enqueue(&mut self, id: I, now: Tick);
    /// 选择下一个可运行任务。
    fn pick_next(&mut self, now: Tick) -> Option<I>;
    /// 时钟中断回调，用于时间片递减和抢占决策。
    fn on_tick(&mut self, current: I, now: Tick) -> SchedDecision<I>;
    /// 当前任务阻塞时的状态更新。
    fn on_block(&mut self, id: I, now: Tick);
    /// 任务被唤醒时的状态更新。
    fn on_wakeup(&mut self, id: I, now: Tick);
    /// 任务退出时清理内部状态（默认可不处理）。
    fn on_exit(&mut self, _id: I, _now: Tick) {}
}

/// 调度事件类型（统一埋点）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    /// 任务进入就绪队列。
    Enqueue,
    /// 任务被选中开始运行。
    Dispatch,
    /// 时钟 tick 到来。
    Tick,
    /// 当前任务被抢占。
    Preempt,
    /// 当前任务阻塞。
    Block,
    /// 某任务被唤醒。
    Wakeup,
    /// 某任务退出。
    Exit,
}

/// 单条调度事件。
#[derive(Clone, Copy, Debug)]
pub struct SchedEvent<I: Copy> {
    /// 时间戳（tick）。
    pub ts: Tick,
    /// 事件种类。
    pub kind: EventKind,
    /// 关联任务。
    pub task: Option<I>,
    /// 事件发生时就绪队列长度。
    pub run_queue_len: usize,
}

/// 单个任务的统计信息。
#[derive(Clone, Debug, Default)]
pub struct TaskStats {
    /// 首次进入就绪队列时间。
    pub first_enqueue_ts: Option<Tick>,
    /// 首次被调度运行时间。
    pub first_run_ts: Option<Tick>,
    /// 任务结束时间。
    pub finish_ts: Option<Tick>,
    /// 累计运行时长（tick）。
    pub run_ticks: u64,
    /// 累计在就绪队列中等待的时长（tick），不含阻塞时间。
    pub wait_ticks: u64,
    /// 被调度（切入）次数，可视作上下文切换次数近似指标。
    pub ctx_switches: u64,
    /// 最近一次唤醒时间，用于计算交互延迟。
    pub last_wakeup_ts: Option<Tick>,
    /// 所有 wakeup->dispatch 延迟样本。
    pub wakeup_latencies: Vec<u64>,
    /// 本次连续等待的起始时间（Enqueue/Preempt/Wakeup 时设置，Dispatch/Block/Exit 时清除）。
    pub last_ready_since: Option<Tick>,
    /// 任务是否处于阻塞态（等 I/O，不在就绪队列中）。
    pub is_blocked: bool,
    /// 饥饿次数（单次连续等待超过阈值算一次）。
    pub starvation_count: u64,
}

/// 实验汇总指标。
#[derive(Clone, Copy, Debug, Default)]
pub struct ExperimentSummary {
    /// 平均等待时间。
    pub avg_wait: f64,
    /// 平均周转时间。
    pub avg_turnaround: f64,
    /// 吞吐量（任务数/时间）。
    pub throughput: f64,
    /// 交互延迟 P95。
    pub p95_latency: u64,
    /// 交互延迟 P99。
    pub p99_latency: u64,
    /// 总饥饿次数。
    pub starvation_total: u64,
}

/// 统一数据采集器（框架版）。
///
/// 你可以在 `record` / `summary` 中补全：
/// - 运行与等待时间累计
/// - wakeup->dispatch 延迟分位数
/// - 饥饿检测逻辑
pub struct MetricsCollector<I: Copy + Ord> {
    /// 原始事件流，可离线分析与回放。
    pub events: Vec<SchedEvent<I>>,
    /// 每任务统计信息。
    pub per_task: BTreeMap<I, TaskStats>,
    /// 最近一次事件时间戳。
    pub last_ts: Tick,
    /// 当前运行任务。
    pub running: Option<I>,
    /// 饥饿阈值（tick）。
    pub starvation_threshold: u64,
}

impl<I: Copy + Ord> MetricsCollector<I> {
    /// 创建采集器。
    pub fn new(starvation_threshold: u64) -> Self {
        Self {
            events: Vec::new(),
            per_task: BTreeMap::new(),
            last_ts: 0,
            running: None,
            starvation_threshold,
        }
    }

    /// 记录单条事件，并更新统计量。
    ///
    /// 时间记账采用"两阶段"：
    /// 1. 结算上一事件到本事件之间的连续时间（delta）
    /// 2. 处理本事件瞬时状态转移
    pub fn record(&mut self, event: SchedEvent<I>) {
        let now = event.ts;
        let delta = now.saturating_sub(self.last_ts);

        // ── 阶段 1：结算 [last_ts, now) 这段连续时间 ──
        if delta > 0 {
            // 正在运行的任务 → 累加 run_ticks
            if let Some(rid) = self.running {
                if let Some(s) = self.per_task.get_mut(&rid) {
                    if s.finish_ts.is_none() {
                        s.run_ticks = s.run_ticks.saturating_add(delta);
                    }
                }
            }
            // 在就绪队列里等待的任务 → 累加 wait_ticks
            // 注意：阻塞态的任务（is_blocked=true）不算等待 CPU
            for (id, s) in self.per_task.iter_mut() {
                if Some(*id) != self.running
                    && !s.is_blocked
                    && s.finish_ts.is_none()
                    && s.last_ready_since.is_some()
                {
                    s.wait_ticks = s.wait_ticks.saturating_add(delta);
                }
            }
        }

        // ── 阶段 2：处理离散事件 ──
        match event.kind {
            EventKind::Enqueue => {
                if let Some(id) = event.task {
                    let s = self.per_task.entry(id).or_default();
                    if s.first_enqueue_ts.is_none() {
                        s.first_enqueue_ts = Some(now);
                    }
                    // 开始一轮新的连续等待
                    s.last_ready_since = Some(now);
                    s.is_blocked = false;
                }
            }

            EventKind::Dispatch => {
                if let Some(id) = event.task {
                    let s = self.per_task.entry(id).or_default();
                    if s.first_run_ts.is_none() {
                        s.first_run_ts = Some(now);
                    }
                    s.ctx_switches = s.ctx_switches.saturating_add(1);

                    // 交互延迟：wakeup → dispatch
                    if let Some(wakeup_ts) = s.last_wakeup_ts.take() {
                        s.wakeup_latencies.push(now.saturating_sub(wakeup_ts));
                    }

                    // 饥饿检测：本次连续等待是否超过阈值
                    if let Some(ready_ts) = s.last_ready_since.take() {
                        let waited = now.saturating_sub(ready_ts);
                        if self.starvation_threshold > 0
                            && waited >= self.starvation_threshold
                        {
                            s.starvation_count =
                                s.starvation_count.saturating_add(1);
                        }
                    }

                    self.running = Some(id);
                }
            }

            EventKind::Tick => {
                // Tick 本身不改变任何任务状态。
                // 时间记账已在阶段 1 完成，此处仅留事件记录。
            }

            EventKind::Preempt => {
                // 被抢占：从运行态 → 回到就绪态
                if let Some(id) = event.task {
                    let s = self.per_task.entry(id).or_default();
                    // 回到就绪队列，开始新一轮连续等待
                    s.last_ready_since = Some(now);
                    s.is_blocked = false;
                    if self.running == Some(id) {
                        self.running = None;
                    }
                }
            }

            EventKind::Block => {
                // 主动阻塞：从运行态 → 阻塞态（等 I/O）
                if let Some(id) = event.task {
                    let s = self.per_task.entry(id).or_default();
                    s.is_blocked = true;
                    s.last_ready_since = None;
                    if self.running == Some(id) {
                        self.running = None;
                    }
                }
            }

            EventKind::Wakeup => {
                // I/O 完成唤醒：从阻塞态 → 就绪态
                if let Some(id) = event.task {
                    let s = self.per_task.entry(id).or_default();
                    s.last_wakeup_ts = Some(now);
                    s.last_ready_since = Some(now);
                    s.is_blocked = false;
                }
            }

            EventKind::Exit => {
                if let Some(id) = event.task {
                    let s = self.per_task.entry(id).or_default();
                    s.finish_ts = Some(now);
                    s.last_ready_since = None;
                    s.is_blocked = false;
                    if self.running == Some(id) {
                        self.running = None;
                    }
                }
            }
        }

        self.last_ts = now;
        self.events.push(event);
    }

    /// 计算实验汇总指标。
    pub fn summary(&self) -> ExperimentSummary {
        let mut total_wait: u64 = 0;
        let mut total_turnaround: u64 = 0;
        let mut finished: u64 = 0;
        let mut starvation_total: u64 = 0;
        let mut all_latencies: Vec<u64> = Vec::new();
        let mut earliest: Option<Tick> = None;
        let mut latest: Option<Tick> = None;

        for s in self.per_task.values() {
            starvation_total = starvation_total.saturating_add(s.starvation_count);
            all_latencies.extend_from_slice(&s.wakeup_latencies);

            if let (Some(enq), Some(fin)) = (s.first_enqueue_ts, s.finish_ts) {
                let turnaround = fin.saturating_sub(enq);
                total_turnaround = total_turnaround.saturating_add(turnaround);
                total_wait = total_wait.saturating_add(s.wait_ticks);
                finished += 1;

                earliest = Some(earliest.map_or(enq, |e: Tick| e.min(enq)));
                latest = Some(latest.map_or(fin, |l: Tick| l.max(fin)));
            }
        }

        let avg_wait = if finished > 0 {
            total_wait as f64 / finished as f64
        } else {
            0.0
        };
        let avg_turnaround = if finished > 0 {
            total_turnaround as f64 / finished as f64
        } else {
            0.0
        };
        let throughput = match (earliest, latest) {
            (Some(e), Some(l)) if l > e => finished as f64 / (l - e) as f64,
            _ => 0.0,
        };

        all_latencies.sort_unstable();
        let percentile = |p: usize| -> u64 {
            if all_latencies.is_empty() {
                return 0;
            }
            let idx = (p * all_latencies.len() / 100).min(all_latencies.len() - 1);
            all_latencies[idx]
        };

        ExperimentSummary {
            avg_wait,
            avg_turnaround,
            throughput,
            p95_latency: percentile(95),
            p99_latency: percentile(99),
            starvation_total,
        }
    }

    /// 记录一次“入队”事件。
    pub fn record_enqueue(&mut self, now: Tick, task: I, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Enqueue,
            task: Some(task),
            run_queue_len,
        });
    }

    /// 记录一次“被调度运行”事件。
    pub fn record_dispatch(&mut self, now: Tick, task: I, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Dispatch,
            task: Some(task),
            run_queue_len,
        });
    }

    /// 记录一次时钟 tick 事件。
    pub fn record_tick(&mut self, now: Tick, current: Option<I>, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Tick,
            task: current,
            run_queue_len,
        });
    }

    /// 记录一次“抢占”事件。
    pub fn record_preempt(&mut self, now: Tick, task: I, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Preempt,
            task: Some(task),
            run_queue_len,
        });
    }

    /// 记录一次“阻塞”事件。
    pub fn record_block(&mut self, now: Tick, task: I, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Block,
            task: Some(task),
            run_queue_len,
        });
    }

    /// 记录一次“唤醒”事件。
    pub fn record_wakeup(&mut self, now: Tick, task: I, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Wakeup,
            task: Some(task),
            run_queue_len,
        });
    }

    /// 记录一次“退出”事件。
    pub fn record_exit(&mut self, now: Tick, task: I, run_queue_len: usize) {
        self.record(SchedEvent {
            ts: now,
            kind: EventKind::Exit,
            task: Some(task),
            run_queue_len,
        });
    }
}

/// FCFS 调度器（先来先服务）骨架。
pub struct FcfsScheduler<I: Copy + Ord> {
    ready: VecDeque<I>,
}

impl<I: Copy + Ord> FcfsScheduler<I> {
    /// 创建 FCFS 调度器。
    pub fn new() -> Self {
        Self {
            ready: VecDeque::new(),
        }
    }

    /// 当前就绪队列长度。
    pub fn len(&self) -> usize {
        self.ready.len()
    }

    /// 就绪队列是否为空。
    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }
}

impl<I: Copy + Ord> Default for FcfsScheduler<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Copy + Ord> PluggableScheduler<I> for FcfsScheduler<I> {
    fn enqueue(&mut self, id: I, _now: Tick) {
        self.ready.push_back(id);
    }

    fn pick_next(&mut self, _now: Tick) -> Option<I> {
        self.ready.pop_front()
    }

    fn on_tick(&mut self, _current: I, _now: Tick) -> SchedDecision<I> {
        SchedDecision::KeepRunning
    }

    fn on_block(&mut self, _id: I, _now: Tick) {}

    fn on_wakeup(&mut self, id: I, now: Tick) {
        self.enqueue(id, now);
    }
}

/// SJF 调度器（最短作业优先，非抢占）骨架。
pub struct SjfScheduler<I: Copy + Ord> {
    ready: Vec<I>,
    /// 任务预计运行时长（你可以在实验中更新预测值）。
    pub predicted_burst: BTreeMap<I, u64>,
}

impl<I: Copy + Ord> SjfScheduler<I> {
    /// 创建 SJF 调度器。
    pub fn new() -> Self {
        Self {
            ready: Vec::new(),
            predicted_burst: BTreeMap::new(),
        }
    }

    /// 更新某任务的预计运行时长。
    pub fn set_predicted_burst(&mut self, id: I, burst: u64) {
        self.predicted_burst.insert(id, burst);
    }

    /// 当前就绪队列长度。
    pub fn len(&self) -> usize {
        self.ready.len()
    }

    /// 返回任务估计时长（无记录时返回默认值）。
    pub fn get_predicted_burst(&self, id: I, default: u64) -> u64 {
        self.predicted_burst.get(&id).copied().unwrap_or(default)
    }
}

impl<I: Copy + Ord> Default for SjfScheduler<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Copy + Ord> PluggableScheduler<I> for SjfScheduler<I> {
    fn enqueue(&mut self, id: I, _now: Tick) {
        self.ready.push(id);
    }

    fn pick_next(&mut self, _now: Tick) -> Option<I> {
        if self.ready.is_empty() {
            return None;
        }
        let mut best_idx = 0;
        let mut best_burst = self.predicted_burst.get(&self.ready[0]).copied().unwrap_or(u64::MAX);
        for (i, id) in self.ready.iter().enumerate().skip(1) {
            let b = self.predicted_burst.get(id).copied().unwrap_or(u64::MAX);
            if b < best_burst {
                best_burst = b;
                best_idx = i;
            }
        }
        Some(self.ready.remove(best_idx))
    }

    fn on_tick(&mut self, _current: I, _now: Tick) -> SchedDecision<I> {
        SchedDecision::KeepRunning
    }

    fn on_block(&mut self, _id: I, _now: Tick) {}

    fn on_wakeup(&mut self, id: I, now: Tick) {
        self.enqueue(id, now);
    }
}

/// RR 调度器（时间片轮转）骨架。
pub struct RrScheduler<I: Copy + Ord> {
    ready: VecDeque<I>,
    /// 固定时间片长度。
    pub quantum: u64,
    /// 当前任务剩余时间片。
    pub remain: u64,
}

impl<I: Copy + Ord> RrScheduler<I> {
    /// 创建 RR 调度器。
    pub fn new(quantum: u64) -> Self {
        Self {
            ready: VecDeque::new(),
            quantum,
            remain: quantum,
        }
    }

    /// 重置当前时间片。
    pub fn reset_slice(&mut self) {
        self.remain = self.quantum;
    }

    /// 当前就绪队列长度。
    pub fn len(&self) -> usize {
        self.ready.len()
    }
}

impl<I: Copy + Ord> PluggableScheduler<I> for RrScheduler<I> {
    fn enqueue(&mut self, id: I, _now: Tick) {
        self.ready.push_back(id);
    }

    fn pick_next(&mut self, _now: Tick) -> Option<I> {
        let next = self.ready.pop_front();
        if next.is_some() {
            self.reset_slice();
        }
        next
    }

    fn on_tick(&mut self, _current: I, _now: Tick) -> SchedDecision<I> {
        if self.remain > 0 {
            self.remain -= 1;
        }
        if self.remain == 0 {
            SchedDecision::Preempt
        } else {
            SchedDecision::KeepRunning
        }
    }

    fn on_block(&mut self, _id: I, _now: Tick) {
        self.reset_slice();
    }

    fn on_wakeup(&mut self, id: I, now: Tick) {
        self.enqueue(id, now);
    }
}

/// MLFQ 调度器（多级反馈队列）骨架。
pub struct MlfqScheduler<I: Copy + Ord, const N: usize> {
    /// 各优先级就绪队列，0 为最高优先级。
    pub queues: [VecDeque<I>; N],
    /// 各级时间片配置。
    pub quanta: [u64; N],
    /// 当前运行任务所属层级。
    pub current_level: usize,
    /// 当前任务剩余时间片。
    pub remain: u64,
    /// 任务当前层级映射。
    pub levels: BTreeMap<I, usize>,
}

impl<I: Copy + Ord, const N: usize> MlfqScheduler<I, N> {
    /// 创建 MLFQ 调度器。
    pub fn new(quanta: [u64; N]) -> Self {
        Self {
            queues: core::array::from_fn(|_| VecDeque::new()),
            quanta,
            current_level: 0,
            remain: quanta[0],
            levels: BTreeMap::new(),
        }
    }

    /// 当前所有队列的总长度。
    pub fn len(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }

    /// 将指定任务提升到最高优先级（用于 anti-starvation）。
    pub fn boost_task(&mut self, id: I) {
        self.levels.insert(id, 0);
        for lv in 1..N {
            if let Some(pos) = self.queues[lv].iter().position(|&x| x == id) {
                self.queues[lv].remove(pos);
                self.queues[0].push_back(id);
                return;
            }
        }
    }

    /// 周期性全局提升：把所有就绪任务移到最高优先级队列。
    pub fn boost_all(&mut self) {
        for lv in 1..N {
            while let Some(id) = self.queues[lv].pop_front() {
                self.levels.insert(id, 0);
                self.queues[0].push_back(id);
            }
        }
    }
}

impl<I: Copy + Ord, const N: usize> PluggableScheduler<I> for MlfqScheduler<I, N> {
    fn enqueue(&mut self, id: I, _now: Tick) {
        let level = self.levels.get(&id).copied().unwrap_or(0).min(N - 1);
        self.levels.insert(id, level);
        self.queues[level].push_back(id);
    }

    fn pick_next(&mut self, _now: Tick) -> Option<I> {
        for lv in 0..N {
            if let Some(id) = self.queues[lv].pop_front() {
                self.current_level = lv;
                self.remain = self.quanta[lv];
                return Some(id);
            }
        }
        None
    }

    fn on_tick(&mut self, current: I, _now: Tick) -> SchedDecision<I> {
        if self.remain > 0 {
            self.remain -= 1;
        }
        if self.remain == 0 {
            let old = self.levels.get(&current).copied().unwrap_or(self.current_level);
            let next = (old + 1).min(N - 1);
            self.levels.insert(current, next);
            SchedDecision::Preempt
        } else {
            SchedDecision::KeepRunning
        }
    }

    fn on_block(&mut self, _id: I, _now: Tick) {}

    fn on_wakeup(&mut self, id: I, now: Tick) {
        self.enqueue(id, now);
    }
}

/// CFS-like 调度器（简化版）。
///
/// 核心思想：每个任务维护 vruntime（虚拟运行时间），每次选 vruntime 最小的任务运行。
/// 权重越大的任务 vruntime 增长越慢，从而获得更多实际 CPU 时间。
pub struct CfsLikeScheduler<I: Copy + Ord> {
    /// 任务虚拟运行时间。
    pub vruntime: BTreeMap<I, u64>,
    /// 任务权重（默认 1024，对应 nice 0）。
    pub weights: BTreeMap<I, u64>,
    /// 就绪任务集合。
    pub ready: Vec<I>,
    /// 最小调度粒度：当前任务至少运行这么多 tick 才考虑抢占。
    pub min_granularity: u64,
    /// 当前任务已连续运行的 tick 数。
    pub current_runtime: u64,
}

/// CFS 默认权重（对应 nice 0）。
const CFS_DEFAULT_WEIGHT: u64 = 1024;

impl<I: Copy + Ord> CfsLikeScheduler<I> {
    /// 创建 CFS-like 调度器。
    pub fn new(min_granularity: u64) -> Self {
        Self {
            vruntime: BTreeMap::new(),
            weights: BTreeMap::new(),
            ready: Vec::new(),
            min_granularity,
            current_runtime: 0,
        }
    }

    /// 设置任务权重（权重越大 → vruntime 增长越慢 → 获得更多 CPU）。
    pub fn set_weight(&mut self, id: I, weight: u64) {
        let w = if weight == 0 { 1 } else { weight };
        self.weights.insert(id, w);
    }

    /// 获取任务权重。
    fn weight(&self, id: I) -> u64 {
        self.weights.get(&id).copied().unwrap_or(CFS_DEFAULT_WEIGHT)
    }

    /// 累计当前任务的 vruntime（按权重缩放）。
    pub fn account_runtime(&mut self, id: I, delta_exec: u64) {
        let w = self.weight(id);
        let scaled = delta_exec.saturating_mul(CFS_DEFAULT_WEIGHT) / w;
        let entry = self.vruntime.entry(id).or_insert(0);
        *entry = entry.saturating_add(scaled);
    }

    /// 就绪队列中 vruntime 最小值。
    fn min_vruntime_in_ready(&self) -> Option<u64> {
        self.ready
            .iter()
            .filter_map(|id| self.vruntime.get(id))
            .copied()
            .min()
    }

    /// 当前就绪队列长度。
    pub fn len(&self) -> usize {
        self.ready.len()
    }
}

impl<I: Copy + Ord> PluggableScheduler<I> for CfsLikeScheduler<I> {
    fn enqueue(&mut self, id: I, _now: Tick) {
        if !self.ready.contains(&id) {
            // 新入队任务的 vruntime 至少对齐到当前最小值，防止新任务饿死老任务
            if let Some(min_vr) = self.min_vruntime_in_ready() {
                let vr = self.vruntime.entry(id).or_insert(0);
                if *vr < min_vr {
                    *vr = min_vr;
                }
            } else {
                self.vruntime.entry(id).or_insert(0);
            }
            self.ready.push(id);
        }
    }

    fn pick_next(&mut self, _now: Tick) -> Option<I> {
        if self.ready.is_empty() {
            return None;
        }
        let mut best_idx = 0;
        let mut best_vr = self.vruntime.get(&self.ready[0]).copied().unwrap_or(0);
        for (i, id) in self.ready.iter().enumerate().skip(1) {
            let vr = self.vruntime.get(id).copied().unwrap_or(0);
            if vr < best_vr {
                best_vr = vr;
                best_idx = i;
            }
        }
        self.current_runtime = 0;
        Some(self.ready.remove(best_idx))
    }

    fn on_tick(&mut self, current: I, _now: Tick) -> SchedDecision<I> {
        self.account_runtime(current, 1);
        self.current_runtime += 1;

        if self.current_runtime < self.min_granularity {
            return SchedDecision::KeepRunning;
        }

        let cur_vr = self.vruntime.get(&current).copied().unwrap_or(0);
        if let Some(min_vr) = self.min_vruntime_in_ready() {
            if min_vr < cur_vr {
                return SchedDecision::Preempt;
            }
        }
        SchedDecision::KeepRunning
    }

    fn on_block(&mut self, _id: I, _now: Tick) {
        self.current_runtime = 0;
    }

    fn on_wakeup(&mut self, id: I, now: Tick) {
        self.enqueue(id, now);
    }

    fn on_exit(&mut self, id: I, _now: Tick) {
        self.vruntime.remove(&id);
        self.weights.remove(&id);
        self.current_runtime = 0;
    }
}
