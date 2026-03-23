# 实验 4：可插拔调度算法实验套件

## 1. 学习目标


| 课程知识点           | 本实验覆盖                                 |
| --------------- | ------------------------------------- |
| 进程调度基本概念        | 调度器统一接口设计                             |
| FCFS / SJF / RR | 三种经典算法完整实现                            |
| MLFQ（多级反馈队列）    | 时间片分级 + 降级/升级策略                       |
| CFS（完全公平调度）     | vruntime + 权重缩放简化版                    |
| 调度指标量化比较        | 等待时间 / 周转时间 / 吞吐量 / P95-P99 延迟 / 饥饿检测 |


## 2. 背景与关键概念

原教程只有一种调度策略（简单 FIFO），无法直观对比不同算法的行为差异。本实验的核心思路：

- **策略与机制分离**：内核只关心 `enqueue / pick_next / on_tick / on_block / on_wakeup` 五个 hook 点，具体策略由实现 `PluggableScheduler` trait 的结构体决定。
- **统一数据采集**：每次状态变化产生 `SchedEvent`，由 `MetricsCollector` 实时记账，实验结束后一次性输出汇总指标。

### 任务生命周期与事件流

```
创建 ──Enqueue──► 就绪 ──Dispatch──► 运行 ──Exit──► 结束
                   ▲                   │
                   │                   ├──Preempt──► 就绪（被抢占）
                   │                   │
                   │                   └──Block──► 阻塞（等 I/O）
                   │                                  │
                   └────────Wakeup────────────────────┘
```

## 3. 实现任务拆解

### 3.1 文件结构

```
exp4-scheduler/
├── Cargo.toml
├── kernel/
│   ├── lib.rs              ← 统一导出
│   ├── scheduler.rs        ← 核心：trait + 五种调度器 + 采集器
│   └── workload.rs         ← 模拟 workload + 自动对比测试引擎
├── user/
│   └── sched_compare.rs    ← 三组 workload 对比测试
├── docs/
│   └── exp4.md             ← 本文档
└── scripts/
    └── run_exp4.sh         ← 一键跑实验
```

### 3.2 接口设计

`**PluggableScheduler<I>` trait**（策略插件接口）：


| 方法                      | 调用时机 | 职责                           |
| ----------------------- | ---- | ---------------------------- |
| `enqueue(id, now)`      | 任务就绪 | 按策略放入内部数据结构                  |
| `pick_next(now)`        | 需要调度 | 选出下一个运行任务                    |
| `on_tick(current, now)` | 时钟中断 | 时间片管理，返回 KeepRunning/Preempt |
| `on_block(id, now)`     | 任务阻塞 | 清理运行状态                       |
| `on_wakeup(id, now)`    | 任务唤醒 | 重新入队                         |
| `on_exit(id, now)`      | 任务退出 | 清理内部状态                       |


`**MetricsCollector<I>`**（统一数据采集器）：

采用"两阶段记账"：

1. **结算连续时间**：每两个事件之间的 delta，运行中的任务累加 `run_ticks`，就绪态任务累加 `wait_ticks`（阻塞态不算）
2. **处理离散事件**：状态转移、饥饿检测（单次连续等待 ≥ 阈值）、交互延迟采样（wakeup → dispatch）

### 3.3 五种调度器关键数据结构


| 调度器          | 核心结构                             | pick_next 策略      | on_tick 行为                                    |
| ------------ | -------------------------------- | ----------------- | --------------------------------------------- |
| **FCFS**     | `VecDeque`                       | pop_front         | KeepRunning（不抢占）                              |
| **SJF**      | `Vec` + `predicted_burst`        | 选最短 burst         | KeepRunning（非抢占）                              |
| **RR**       | `VecDeque` + `quantum/remain`    | pop_front + 重置时间片 | remain-=1，到 0 则 Preempt                       |
| **MLFQ**     | `[VecDeque; N]` + `quanta[N]`    | 从高到低找非空队列         | 到期降级 + Preempt                                |
| **CFS-like** | `BTreeMap<vruntime>` + `weights` | 选最小 vruntime      | vruntime += delta×(1024/weight)，超过最小则 Preempt |


## 4. 常见 bug 清单


| 症状                | 原因                     | 定位方法                     |
| ----------------- | ---------------------- | ------------------------ |
| 所有调度器指标完全相同       | workload 太简单（只有 1 个任务） | 增加并发任务数                  |
| CFS 新任务饿死老任务      | 新任务 vruntime=0 远小于老任务  | enqueue 时对齐 min_vruntime |
| MLFQ 低优先级永远不运行    | 缺少 boost_all 周期提升      | 加饥饿检测 + 定期 boost         |
| wait_ticks 包含阻塞时间 | 阻塞态任务也在累加              | 检查 `is_blocked` 过滤       |
| 饥饿计数异常偏高          | 用累计等待而非单次连续等待          | 改用 `last_ready_since`    |


## 5. 测试与指标

### 运行实验

```bash
cd exp4-scheduler

# 一键运行
bash scripts/run_exp4.sh

# 或直接 cargo test
cargo test --test sched_compare -- --nocapture
```

### 三组 workload


| Workload  | 任务特征                     | 考察重点         |
| --------- | ------------------------ | ------------ |
| CPU-bound | 4 个纯计算任务，burst 20~50     | 等待时间、周转时间差异  |
| IO-bound  | 3 个频繁 I/O 任务，短 CPU + 长等待 | 交互延迟 P95/P99 |
| Mixed     | CPU + IO + 交互型混合         | 公平性、饥饿、综合表现  |


### 输出样例（Mixed workload）

```
Scheduler     AvgWait AvgTurnaround Throughput   P95Lat   P99Lat Starvation
------------------------------------------------------------------------
FCFS            72.60       110.80     0.0329       43       43          4
SJF             69.80       108.00     0.0360       56       56          4
RR-10           61.80        98.20     0.0407       23       23          0
MLFQ-3          36.20        72.80     0.0407        7        7          0
CFS             38.40        72.40     0.0407        9        9          0
```

## 6. 思考题

1. **公平性 vs 效率**：SJF 周转时间最优，但可能饿死长任务。如何量化"公平"？
2. **MLFQ 参数敏感性**：改变各层时间片比例（如 [2,4,8] vs [8,16,32]），对交互延迟影响多大？
3. **CFS 权重实验**：给 CPU-bound 任务设低权重、IO-bound 设高权重，P99 延迟能降多少？
4. **负载突变**：在运行中途突然加入 10 个 CPU-bound 任务，各调度器的响应时间如何变化？
5. **真实内核集成**：如果要把 `PluggableScheduler` 接入 rCore 的 `TaskManager`，需要在哪些地方加 hook？

