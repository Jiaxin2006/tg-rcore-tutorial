# exp4-scheduler

`exp4-scheduler` 是实验 4 的调度算法实验套件。

它包含两层能力：

- 调度策略层：`FCFS / SJF / RR / MLFQ / CFS-like`
- 内核接入层：`KernelSchedulerRuntime`

如果只想在宿主机上比较不同调度算法，可以直接跑 `workload` 模拟。
如果后续要把实验结果接进 rCore 风格内核，核心入口就是 `KernelSchedulerRuntime`。

## KernelSchedulerRuntime 接口

`KernelSchedulerRuntime<I, S>` 是一个“调度运行时包装器”，用于把实验里的
`PluggableScheduler<I>` 变成可以嵌入内核调度路径的组件。

它的职责很明确：

- 持有一个具体调度策略 `policy: S`
- 维护当前运行任务 `current`
- 维护逻辑时钟 `now`
- 维护 ready 队列长度近似值 `ready_len`
- 统一记录调度事件并汇总指标 `metrics`

它**不负责**：

- 保存任务/线程实体
- 做上下文切换
- 管理地址空间、寄存器、文件描述符等内核资源

这些仍然应该由内核自己的 `TaskManager`、`Processor`、`ThreadManager` 一类结构负责。

## 为什么需要它

当前 `tg-rcore-tutorial-ch5` 到 `ch8` 的内核调用风格大体一致：

- 新任务进入就绪队列：`add(...)`
- 取下一个任务运行：`fetch()` / `find_next()`
- 当前任务让出 CPU：`make_current_suspend()`
- 当前任务阻塞：`make_current_blocked()`
- 阻塞任务唤醒：`re_enque(...)`
- 当前任务退出：`make_current_exited(...)`

`KernelSchedulerRuntime` 的设计就是为了对齐这类接口风格，同时把真正的调度逻辑转发到：

- `enqueue`
- `pick_next`
- `on_tick`
- `on_block`
- `on_wakeup`
- `on_exit`

这样内核机制和调度策略就能解耦。

## 它和内核生命周期的对应关系

`KernelSchedulerRuntime` 提供的核心方法如下：

- `add_task(id)` / `add(id)`
  用于新任务进入 ready 队列
- `dispatch_next()` / `fetch()`
  用于选择下一个运行任务
- `make_current_suspend()`
  用于当前任务主动让出 CPU，或被抢占后重新入队
- `make_current_blocked()`
  用于当前任务进入阻塞态
- `re_enque(id)`
  用于阻塞任务被唤醒后重新回到 ready 队列
- `make_current_exited()`
  用于当前任务退出
- `on_tick()`
  用于把时钟 tick 转发给策略对象，并返回 `SchedDecision`

这意味着后续接入真实内核时，通常只需要把原来直接操作 `ready_queue` 的代码，替换成对这些方法的调用。

## 一个最小例子

```rust
use exp4_scheduler::{FcfsScheduler, KernelSchedulerRuntime, Schedule};

let mut sched = KernelSchedulerRuntime::new(FcfsScheduler::<usize>::new(), 50);

sched.add(1);
sched.add(2);

let first = sched.fetch();
assert_eq!(first, Some(1));

sched.advance_ticks(3);
sched.make_current_suspend();

let second = sched.fetch();
assert_eq!(second, Some(2));
```

如果换成抢占式策略：

```rust
use exp4_scheduler::{KernelSchedulerRuntime, RrScheduler, SchedDecision};

let mut sched = KernelSchedulerRuntime::new(RrScheduler::<usize>::new(2), 50);
sched.add_task(1);
sched.add_task(2);
sched.dispatch_next();

sched.advance_ticks(1);
assert_eq!(sched.on_tick(), Some(SchedDecision::KeepRunning));

sched.advance_ticks(1);
assert_eq!(sched.on_tick(), Some(SchedDecision::Preempt));
```

## 运行实验

直接运行三组 workload 的对比实验：

```bash
cd exp4-scheduler
bash scripts/run_exp4.sh
```

或者直接用 Cargo：

```bash
cd exp4-scheduler
cargo test --test sched_compare -- --nocapture --test-threads=1
```

如果只想验证库本身：

```bash
cargo test
```

## 现在的定位

当前 `exp4-scheduler` 仍然是一个独立实验 crate，但它已经不只是“宿主机模拟器”了。

现在它同时提供：

- 可比较的调度策略实现
- 统一指标采集器 `MetricsCollector`
- 可嵌入内核的桥接层 `KernelSchedulerRuntime`

因此后续真正接入 `tg-task-manage` 或某个 `tg-rcore-tutorial-chx` 时，推荐的方向是：

- 由内核 crate 依赖 `exp4-scheduler`
- 用 `KernelSchedulerRuntime` 替换原始 `ready_queue + add/fetch`
- 保留内核已有的任务实体与上下文切换逻辑

完整实验说明见 [`docs/exp4.md`](docs/exp4.md)。
