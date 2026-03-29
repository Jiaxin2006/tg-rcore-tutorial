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
├── README.md              ← 仓库入口说明，包含 KernelSchedulerRuntime 介绍
├── Cargo.toml
├── src/
│   ├── lib.rs              ← 统一导出
│   ├── kernel.rs           ← 内核接入层：KernelSchedulerRuntime
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

## 4. 当前状态与“如何接入内核”

### 4.1 当前状态：还没有被真实内核实际接入，但已经有接入层实现

当前 `exp4-scheduler` 仍然是一个**独立实验 crate**，但它已经不只是纯模拟框架了，当前包含：

- 在 [`src/kernel.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/exp4-scheduler/src/kernel.rs) 中实现 `KernelSchedulerRuntime`
- 在 [`src/scheduler.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/exp4-scheduler/src/scheduler.rs) 中定义统一调度接口 `PluggableScheduler`
- 在 [`src/workload.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/exp4-scheduler/src/workload.rs) 中用模拟 workload 驱动调度器
- 用 `MetricsCollector` 收集等待时间、周转时间、吞吐量、延迟和饥饿指标

也就是说，它现在已经完成了三层内容：

- **策略插件**：`PluggableScheduler`
- **统一统计**：`MetricsCollector`
- **内核桥接层**：`KernelSchedulerRuntime`

不过它**还没有真正被某个 `tg-rcore-tutorial-chx` 内核 crate 接到上下文切换路径上**。

### 4.2 “接入内核”到底接什么

接入的不是某个具体章节代码，而是把你设计的调度接口接到内核里这五类事件上：

| 内核事件 | 对应 hook | 含义 |
| ------- | ------- | ---- |
| 新任务进入就绪队列 | `enqueue(id, now)` | 新建 / fork / 被抢占后重新就绪 |
| 需要挑选下一个运行任务 | `pick_next(now)` | 调度点真正从就绪队列里选人 |
| 时钟中断到来 | `on_tick(current, now)` | 时间片递减、是否抢占 |
| 当前任务阻塞 | `on_block(id, now)` | 等待 I/O / 锁 / 条件变量 |
| 阻塞任务被唤醒 | `on_wakeup(id, now)` | 从等待队列回到就绪队列 |
| 任务退出 | `on_exit(id, now)` | 清理内部状态 |

核心思想是：**上下文切换和任务状态维护属于内核机制，队列组织和抢占策略属于调度器插件。**

### 4.3 接入步骤

#### 第 1 步：先把“机制”和“策略”拆开

内核里至少要有两层：

1. **任务管理器**：负责保存 TCB/PCB、维护任务状态、执行上下文切换
2. **调度器对象**：只负责“谁进队、谁出队、何时抢占”

当前仓库里已经有一个可直接复用的接入层：

```rust
pub struct KernelSchedulerRuntime<I, S>
where
    I: Copy + Ord,
    S: PluggableScheduler<I>,
{
    pub policy: S,
    pub metrics: MetricsCollector<I>,
    now: Tick,
    current: Option<I>,
    ready_len: usize,
}
```

这里 `policy` 只做策略决策，`metrics` 负责统一埋点；真正的任务实体仍然放在内核自己的任务表中。  
`KernelSchedulerRuntime` 还额外维护了：

- `current`：当前正在运行的任务 ID
- `now`：逻辑时钟
- `ready_len`：ready 队列长度近似值，用于埋点

#### 第 2 步：把“任务变成就绪态”的所有入口统一走 `enqueue`

凡是任务重新进入 ready queue 的地方，都不要直接 `push_back`，而是统一改成调用接入层：

```rust
sched.add_task(id);
// 或为了兼容旧的 add/fetch 风格，直接 sched.add(id)
```

通常包括：

- `spawn`
- `fork`
- 新线程创建
- 被抢占后回到就绪态
- `sleep`/I/O 完成后的唤醒
- 锁、信号量、条件变量唤醒

这样做的意义是：以后无论换成 FCFS、RR、MLFQ 还是 CFS-like，内核机制层都不用重写。

#### 第 3 步：把调度点统一走 `pick_next`

原来内核常见写法是：

```rust
let next = ready_queue.pop_front();
```

要改成调用接入层：

```rust
let next = sched.dispatch_next();
// 或为了兼容旧接口，直接 sched.fetch()
```

这一步是整个接入的关键，因为它把“下一个运行谁”完全交给策略插件决定。

#### 第 4 步：在时钟中断里接 `on_tick`

抢占式调度器（RR / MLFQ / CFS-like）必须在 timer interrupt 中触发。  
`KernelSchedulerRuntime` 已经提供了现成的 `on_tick()`：

```rust
sched.set_now(now);
match sched.on_tick() {
    Some(SchedDecision::KeepRunning) | None => {}
    Some(SchedDecision::Preempt) => {
        sched.make_current_suspend();
        let next = sched.dispatch_next();
        // 然后由内核自己的上下文切换逻辑切到 next
    }
    Some(SchedDecision::SwitchTo(next)) => {
        sched.preempt_current();
        // 然后由内核自己的上下文切换逻辑切到 next
    }
}
```

如果没有这一步，那么 RR / MLFQ / CFS-like 实际上都会退化成“只会排队、不会按 tick 抢占”的假实现。

#### 第 5 步：在阻塞路径接 `on_block`

当前任务因为 I/O、锁、信号量、条件变量等原因不能继续运行时，调用：

```rust
sched.make_current_blocked();
```

这里有个原则：**阻塞时不要把任务重新塞回 ready queue**，否则等待 I/O 的时间会被错误地统计成等待 CPU。

#### 第 6 步：在唤醒路径接 `on_wakeup`

某个阻塞任务可以重新运行时，调用：

```rust
sched.re_enque(id);
```

对于 FCFS / RR 这类简单实现，`on_wakeup` 内部通常只是再次 `enqueue`；  
对于 MLFQ / CFS-like，唤醒时可能还会涉及优先级层级或 vruntime 修正。

#### 第 7 步：在退出路径接 `on_exit`

任务退出时，调用：

```rust
sched.make_current_exited();
```

否则调度器内部可能残留该任务的时间片、优先级、vruntime 或 burst 预测信息。

#### 第 8 步：把统计采集放在“状态边界”而不是“算法内部”

统一数据采集应由内核调度框架负责，而不是散落在每个策略实现里。推荐规则：

- 进入 ready：`record_enqueue`
- 真正开始运行：`record_dispatch`
- 每次 tick：`record_tick`
- 被抢占：`record_preempt`
- 主动阻塞：`record_block`
- 阻塞后唤醒：`record_wakeup`
- 运行结束：`record_exit`

这样不同策略跑出的指标才有可比性。

### 4.4 一个最小接入骨架

当前代码里已经有一个比“手写骨架”更直接的版本，也就是 `KernelSchedulerRuntime`。  
如果你想把它挂进 rCore 风格内核，最小调用骨架大致会是：

```rust
let mut sched = KernelSchedulerRuntime::new(FcfsScheduler::<TaskId>::new(), 50);

// 新任务就绪
sched.add_task(id);

// 选下一个
let next = sched.dispatch_next();

// 当前任务让出 CPU
sched.make_current_suspend();

// 当前任务阻塞
sched.make_current_blocked();

// 阻塞任务唤醒
sched.re_enque(id);

// 当前任务退出
sched.make_current_exited();
```

可以把它理解成：

- `KernelSchedulerRuntime` 负责“把内核生命周期事件翻译成调度 hook”
- 内核自己的 `Processor/TaskManager` 仍负责“任务实体管理 + 上下文切换”

### 4.5 接入后的验收标准

如果你真的把接口接进内核，至少应满足下面几个现象：

1. 把 `policy` 从 `FcfsScheduler` 切到 `RrScheduler` 时，不改上下文切换代码也能运行
2. `Rr` 的时间片耗尽后会被抢占，而 `Fcfs` 不会
3. `MLFQ` 的交互型任务延迟显著低于 `FCFS`
4. `CFS-like` 不会因为新任务加入就让老任务长期饿死
5. 实验报表只依赖统一事件流，不依赖某个调度器私有逻辑


## 5. 常见 bug 清单


| 症状                | 原因                     | 定位方法                     |
| ----------------- | ---------------------- | ------------------------ |
| 所有调度器指标完全相同       | workload 太简单（只有 1 个任务） | 增加并发任务数                  |
| CFS 新任务饿死老任务      | 新任务 vruntime=0 远小于老任务  | enqueue 时对齐 min_vruntime |
| MLFQ 低优先级永远不运行    | 缺少 boost_all 周期提升      | 加饥饿检测 + 定期 boost         |
| wait_ticks 包含阻塞时间 | 阻塞态任务也在累加              | 检查 `is_blocked` 过滤       |
| 饥饿计数异常偏高          | 用累计等待而非单次连续等待          | 改用 `last_ready_since`    |


## 6. 测试与指标

### 运行实验

```bash
cd exp4-scheduler

# 一键运行
bash scripts/run_exp4.sh

# 或直接 cargo test
cargo test --test sched_compare -- --nocapture --test-threads=1

# 或跑全部测试（包括 KernelSchedulerRuntime 单元测试）
cargo test
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

## 7. 思考题

1. **公平性 vs 效率**：SJF 周转时间最优，但可能饿死长任务。如何量化"公平"？
2. **MLFQ 参数敏感性**：改变各层时间片比例（如 [2,4,8] vs [8,16,32]），对交互延迟影响多大？
3. **CFS 权重实验**：给 CPU-bound 任务设低权重、IO-bound 设高权重，P99 延迟能降多少？
4. **负载突变**：在运行中途突然加入 10 个 CPU-bound 任务，各调度器的响应时间如何变化？
5. **真实内核集成**：如果要把 `PluggableScheduler` 接入 rCore 的 `TaskManager`，需要在哪些地方加 hook？
