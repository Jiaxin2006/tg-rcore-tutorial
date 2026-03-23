//! 调度实验模拟 workload 与自动化对比测试。
//!
//! 本模块不依赖内核，可直接 `cargo test` 在宿主机上验证五种调度算法的正确性与指标。
//! 同时也是实验交付的核心"用户态 workload 模拟"。

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::scheduler::*;

/// 模拟任务描述。
#[derive(Clone, Debug)]
pub struct SimTask {
    /// 任务名称（用于报表）。
    pub name: &'static str,
    /// 到达时间（tick）。
    pub arrive: Tick,
    /// CPU burst 列表与 I/O burst 交替：[cpu, io, cpu, io, ..., cpu]。
    /// 奇数位置是 CPU burst，偶数位置是 I/O 等待时长。
    pub bursts: Vec<u64>,
}

/// 三类典型 workload 工厂。
pub fn cpu_bound_workload() -> Vec<SimTask> {
    vec![
        SimTask { name: "cpu-A", arrive: 0, bursts: vec![50] },
        SimTask { name: "cpu-B", arrive: 2, bursts: vec![40] },
        SimTask { name: "cpu-C", arrive: 4, bursts: vec![30] },
        SimTask { name: "cpu-D", arrive: 6, bursts: vec![20] },
    ]
}

/// I/O 密集型 workload：频繁短 CPU burst + 长 I/O 等待。
pub fn io_bound_workload() -> Vec<SimTask> {
    vec![
        SimTask { name: "io-A", arrive: 0, bursts: vec![3, 10, 3, 10, 3] },
        SimTask { name: "io-B", arrive: 1, bursts: vec![2, 15, 2, 15, 2] },
        SimTask { name: "io-C", arrive: 2, bursts: vec![4, 8, 4, 8, 4] },
    ]
}

/// 混合 workload：CPU 密集 + I/O 密集 + 交互型任务混合。
pub fn mixed_workload() -> Vec<SimTask> {
    vec![
        SimTask { name: "cpu-1",  arrive: 0,  bursts: vec![60] },
        SimTask { name: "io-1",   arrive: 0,  bursts: vec![3, 12, 3, 12, 3] },
        SimTask { name: "inter-1",arrive: 1,  bursts: vec![1, 5, 1, 5, 1, 5, 1] },
        SimTask { name: "cpu-2",  arrive: 5,  bursts: vec![45] },
        SimTask { name: "io-2",   arrive: 8,  bursts: vec![2, 20, 2, 20, 2] },
    ]
}

/// 任务运行时状态。
#[derive(Clone)]
struct TaskSim {
    id: usize,
    burst_idx: usize,
    burst_remain: u64,
    io_finish_at: Option<Tick>,
    finished: bool,
}

/// 驱动一次完整的调度模拟，返回采集器。
pub fn run_simulation(
    sched: &mut dyn PluggableScheduler<usize>,
    tasks: &[SimTask],
    starvation_threshold: u64,
) -> MetricsCollector<usize> {
    let mut collector = MetricsCollector::new(starvation_threshold);
    let mut sims: Vec<TaskSim> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| TaskSim {
            id: i,
            burst_idx: 0,
            burst_remain: t.bursts[0],
            io_finish_at: None,
            finished: false,
        })
        .collect();

    let mut now: Tick = 0;
    let mut running: Option<usize> = None;
    let max_tick: Tick = 2000;

    while now < max_tick {
        // 1. 到达的任务入队
        for (i, t) in tasks.iter().enumerate() {
            if t.arrive == now && !sims[i].finished && sims[i].io_finish_at.is_none() {
                let qlen = queue_len_approx(&sims, running);
                sched.enqueue(i, now);
                collector.record_enqueue(now, i, qlen);
            }
        }

        // 2. I/O 完成的任务唤醒
        let wakeup_ids: Vec<usize> = sims
            .iter()
            .filter(|s| s.io_finish_at.map_or(false, |fin| now >= fin))
            .map(|s| s.id)
            .collect();
        for wid in wakeup_ids {
            let s = &mut sims[wid];
            s.io_finish_at = None;
            s.burst_idx += 1;
            if s.burst_idx < tasks[wid].bursts.len() {
                s.burst_remain = tasks[wid].bursts[s.burst_idx];
                let qlen = queue_len_approx(&sims, running);
                sched.on_wakeup(wid, now);
                collector.record_wakeup(now, wid, qlen);
            } else {
                s.finished = true;
                let qlen = queue_len_approx(&sims, running);
                collector.record_exit(now, wid, qlen);
            }
        }

        // 3. 如果没有正在运行的任务，调度下一个
        if running.is_none() {
            if let Some(next) = sched.pick_next(now) {
                running = Some(next);
                let qlen = queue_len_approx(&sims, running);
                collector.record_dispatch(now, next, qlen);
            }
        }

        // 4. 当前运行的任务执行 1 tick
        if let Some(rid) = running {
            let sim = &mut sims[rid];
            sim.burst_remain = sim.burst_remain.saturating_sub(1);

            if sim.burst_remain == 0 {
                let next_idx = sim.burst_idx + 1;
                if next_idx >= tasks[rid].bursts.len() {
                    sim.finished = true;
                    sim.burst_idx = next_idx;
                    let qlen = queue_len_approx(&sims, running);
                    sched.on_exit(rid, now);
                    collector.record_exit(now, rid, qlen);
                    running = None;
                } else {
                    let io_duration = tasks[rid].bursts[next_idx];
                    sim.io_finish_at = Some(now + io_duration);
                    sim.burst_idx = next_idx;
                    let qlen = queue_len_approx(&sims, running);
                    sched.on_block(rid, now);
                    collector.record_block(now, rid, qlen);
                    running = None;
                }
            } else {
                let qlen = queue_len_approx(&sims, running);
                let decision = sched.on_tick(rid, now);
                collector.record_tick(now, Some(rid), qlen);
                if let SchedDecision::Preempt = decision {
                    sched.enqueue(rid, now);
                    collector.record_preempt(now, rid, qlen);
                    running = None;
                }
            }
        }

        // 5. 所有任务完成则提前退出
        if sims.iter().all(|s| s.finished) {
            break;
        }

        now += 1;
    }

    collector
}

fn queue_len_approx(
    sims: &[TaskSim],
    running: Option<usize>,
) -> usize {
    sims.iter()
        .filter(|s| !s.finished && s.io_finish_at.is_none() && Some(s.id) != running)
        .count()
}

/// 格式化打印实验对比结果。
pub fn format_comparison(results: &[(&str, ExperimentSummary)]) -> String {
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "{:<10} {:>10} {:>12} {:>10} {:>8} {:>8} {:>10}\n",
        "Scheduler", "AvgWait", "AvgTurnaround", "Throughput", "P95Lat", "P99Lat", "Starvation"
    ));
    out.push_str(&alloc::format!("{}\n", "-".repeat(72)));
    for (name, s) in results {
        out.push_str(&alloc::format!(
            "{:<10} {:>10.2} {:>12.2} {:>10.4} {:>8} {:>8} {:>10}\n",
            name, s.avg_wait, s.avg_turnaround, s.throughput,
            s.p95_latency, s.p99_latency, s.starvation_total
        ));
    }
    out
}

/// 一键跑全部五种调度器 × 一组 workload，返回对比结果。
pub fn run_all_schedulers(tasks: &[SimTask], starvation_threshold: u64) -> Vec<(&'static str, ExperimentSummary)> {
    let mut results = Vec::new();

    // FCFS
    let mut fcfs = FcfsScheduler::<usize>::new();
    let c = run_simulation(&mut fcfs, tasks, starvation_threshold);
    results.push(("FCFS", c.summary()));

    // SJF (用 burst 总和作为 predicted_burst)
    let mut sjf = SjfScheduler::<usize>::new();
    for (i, t) in tasks.iter().enumerate() {
        let total: u64 = t.bursts.iter().step_by(2).sum();
        sjf.set_predicted_burst(i, total);
    }
    let c = run_simulation(&mut sjf, tasks, starvation_threshold);
    results.push(("SJF", c.summary()));

    // RR (quantum = 10)
    let mut rr = RrScheduler::<usize>::new(10);
    let c = run_simulation(&mut rr, tasks, starvation_threshold);
    results.push(("RR-10", c.summary()));

    // MLFQ (3 levels: 4, 8, 16)
    let mut mlfq = MlfqScheduler::<usize, 3>::new([4, 8, 16]);
    let c = run_simulation(&mut mlfq, tasks, starvation_threshold);
    results.push(("MLFQ-3", c.summary()));

    // CFS-like (min_granularity = 4)
    let mut cfs = CfsLikeScheduler::<usize>::new(4);
    let c = run_simulation(&mut cfs, tasks, starvation_threshold);
    results.push(("CFS", c.summary()));

    results
}
