# T4 实现计划：在 ch8 + doom 基础上支持内核态响应中断与多核处理

目标
在已完成的tg-rcore-tutorial-lab8+doom-game的基础上，扩展如下功能
- 支持内核态响应中断
- 支持多核处理
- 设计用户态测试用例来测试上述功能的正确性
- 设计相对复杂的用户态应用能比较好地展现有了上面的功能后带来的性能变化
提示
- 可以从tg-rcore-tutorial-lab1开始逐步实现内核响应中断和多核支持，并设计测试用例确保功能正确和性能提升（比较慢，比较循序渐进）。当然，也可直接在你的tg-rcore-tutorial-lab8+doom-game repo上进行内核态响应中断和多核支持的设计与实现（相信自己与大模型合作的能力提升）。
- https://github.com/arceos-org/arceos 支持多核，是一个可以学习和参考的对象。
- 要求能理解你和大模型合作后的成果，特别是内核态响应中断的机制和多核处理的机制。

## 1. 目标与现状

当前基线是“已完成的 `tg-rcore-tutorial-lab8 + doom-game`”。T4 需要在此基础上进一步做到：

- 支持**内核态响应中断**
- 支持**多核处理**
- 设计**用户态测试用例**验证上述功能正确性
- 设计一个**相对复杂的用户态应用**，能明显展示这些能力带来的性能变化

推荐路线不是“完全回到 ch1 重做一遍”，而是：

1. 以当前 `tg-rcore-tutorial-ch8` 为主开发分支
2. 借用 `ch3` 的时钟中断经验、`ch5/ch8` 的调度与线程框架
3. 在关键节点做“小范围回退验证”，确认新机制没有破坏既有章节语义

这样做的原因是：T4 的新增点主要是“Trap/中断处理路径”和“多核执行模型”，而不是重新学习单核内核的全部基础设施。

## 2. 最终交付物

建议把 T4 的最终成果组织成 4 类：

1. 内核代码
   - 支持 S 态内核执行期间仍可安全处理时钟/外部中断
   - 支持多 hart 启动、每核陷入、每核调度、每核当前线程
2. 用户态测试程序
   - 正确性测试：验证“中断没有丢”“多核确实并行”“共享数据同步正确”
   - 对照测试：单核 vs 多核、关中断路径 vs 开中断路径
3. 性能展示程序
   - 一个比 microbenchmark 更复杂的用户态应用，能测出多核与中断响应带来的收益
4. 文档
   - 机制说明
   - 实现步骤
   - 测试方法
   - 性能结果与分析

## 3. 建议的总体策略

推荐采用“先把内核变成可安全并发的，再真正打开多核”的顺序：

1. 先整理单核下的中断与锁语义
2. 再做每核局部状态拆分
3. 再做次核启动与并行调度
4. 最后补用户态测试与性能展示

核心原则：

- 先保证**可证明正确**，再追求性能
- 先把“单核下内核态中断响应”做扎实，再开 SMP
- 能用“每核本地数据”解决的问题，不先上全局大锁
- 测试分成“功能正确”和“性能提升”两条线，不混在一起

## 3.1 当前建议的多核落地清单

结合当前仓库状态，建议把“多核处理”拆成下面 4 个可验收里程碑：

1. M1：让 2 个 hart 都能安全进入内核
   - 在 M 态入口和 S 态入口尽早读取 `mhartid`
   - `boot hart` 只做一次性的全局初始化
   - `secondary hart` 不重复清 BSS / 不重复建堆 / 不重复初始化设备
   - 建一个最小 barrier，确保次核完成“本核 trap + 本核 timer + 本核栈/portal 槽位”初始化后再参与系统运行
   - 当前最小 SBI 还没有 HSM/IPI，第一版优先尝试“多个 hart 都进入 `_m_start`，由软件 barrier 收敛”；如果实测 QEMU 不是这样，再补最小 HSM `hart_start`
   - 当前实现状态（2026-03-31）：这一版已经完成“boot hart 一次性初始化 + secondary hart 安全 online”，次核当前只激活内核页表并停在 `wfi` 循环，还**不会参与共享调度器**
   - 当前验证命令：
     - `cd tg-rcore-tutorial-ch8 && TG_CH8_INIT_APP=kernel_interrupt_check cargo run --features exercise`
     - `cd tg-rcore-tutorial-ch8 && TG_CH8_INIT_APP=ch8_usertest cargo run --features exercise`
   - 当前验证现象：
     - 启动日志里能看到 `hart0 online (boot)`、`boot hart released secondary harts`、`hart1 online (secondary)`
     - `kernel_interrupt_check` 在 `-smp 2` 下仍然能看到 `kernel timer interrupt observed` 和 `kernel_interrupt_check: success observed=2`
     - `ch8_usertest` 在 `-smp 2` 下仍能跑到 `ch8 Usertests passed!`

2. M2：把单核全局状态拆成 per-hart
   - `PROCESSOR/current/need_resched/timer_ticks/kernel_timer_interrupts` 改成“每核一份”
   - trap 入口、timer 设定、portal slot 都按 hart 区分
   - 先允许“全局 ready queue + 每核 current”的保守方案，不急着一开始就做每核 run queue
   - 当前实现状态（2026-03-31）：已经把 `PROCESSOR.current/need_resched/timer_ticks/kernel_timer_interrupts` 拆成 per-hart，本地 timer 改成按 `mhartid` 写各自的 `mtimecmp`，secondary hart 也会独立开启 `stimer + kernel trap`
   - 当前还没完成的部分：secondary hart 目前仍主要维护本地 timer/idle 状态，还不会真正并行跑用户线程；要进入 M3，还需要给共享调度器与同步原语补上真正的 SMP 保护
   - 当前验证命令：
     - `cd tg-rcore-tutorial-ch8 && TG_CH8_INIT_APP=kernel_smp_check cargo run --features exercise`
     - `cd tg-rcore-tutorial-ch8 && TG_CH8_INIT_APP=kernel_interrupt_check cargo run --features exercise`
   - 当前验证现象：
     - `kernel_smp_check` 会打印两次快照，能看到 `online_mask=0x3`
     - 快照里 `hart0 delta: ticks>0`、`hart1 delta: ticks>0 kernel_timer_interrupts>0`
     - 日志里仍能看到 `kernel_interrupt_check: hart0 success observed=2`

3. M3：真正让多核并行跑线程
   - 允许两个 hart 都从 ready queue 取任务
   - 唤醒/退出/阻塞路径先保证正确，再优化负载均衡
   - 第一版可以先不做 IPI，只依赖 timer tick 驱动其他 hart 尽快看到 `need_resched`
   - 如果发现跨核唤醒延迟太高，再增加“目标 hart 的事件标记 + MSIP/IPI”
   - 当前实现状态（2026-03-31）：已经落地“全局 ready queue + 每核 current”的第一版共享调度器；secondary hart 会进入同一套 `scheduler_loop`，`portal` 改成按 hart 分 slot，线程管理器也增加了 `Runnable / Running(hart_id) / Blocked` 状态来避免重复调度
   - 当前 SMP 保护策略：采用 `KERNEL_BIG_LOCK` 作为第一版大内核锁，保证共享内核状态一次只由一个 hart 修改；切到用户态前释放，trap 回内核后重新获取
   - 当前验证现象：
     - `kernel_smp_check` 中能看到 `user observed hart mask=0x3`
     - `ch8_usertest` 在共享调度器下完整通过
     - `tg-rcore-tutorial-checker --ch 8 --exercise` 结果为 `PASS 25/25`

4. M4：补同步与性能展示
   - 检查所有全局共享结构是否需要 `SpinLock + 关本核中断` 或其他保护
   - 做单核/双核对照实验，确认不是“能跑”而是真有并行收益

建议的验收标准：

- M1 通过：`-smp 2` 时能稳定看到 `hart0 online`、`hart1 online`，且不会双重初始化
- M2 通过：每个 hart 的 timer tick 都单调增长，`current thread` 不再是全局唯一变量
- M3 通过：用户态多线程程序能同时跑在不同 hart 上，线程退出/阻塞/唤醒不丢失
- M4 通过：多核正确性测试稳定通过，并且至少一个 workload 在 2 hart 下明显快于 1 hart

## 4. 分阶段计划

## Phase 0：冻结基线与测量基线

目标：先得到一个可重复的对照基线，避免后面“变快/变慢”没有参照。

需要完成：

- 固定当前 `ch8 + doom` 可运行状态
- 记录单核下已有测试是否通过
- 记录 Doom 或其他图形程序的基线帧率/耗时
- 记录线程、锁、文件系统、framebuffer 路径的已有行为

建议输出：

- `docs/t4-plan.md` 之外，再保留一份实验日志
- 基线命令、QEMU 参数、测量口径统一下来
- 当前仓库已补充基线材料：
  - [`docs/t4-phase0-baseline.md`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/docs/t4-phase0-baseline.md)
  - [`scripts/t4-phase0-baseline.sh`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/scripts/t4-phase0-baseline.sh)

建议指标：

- Shell 启动时间
- Doom 首帧时间
- 连续 N 帧平均绘制时间
- 用户线程并发测试总耗时

## Phase 1：支持内核态响应中断

### 1.1 目标

让内核在执行 syscall、调度、设备处理等 S 态代码时，不是“全程闷头跑到返回用户态才处理时钟”，而是能在满足安全约束的前提下正常响应中断。

### 1.2 机制设计

需要先明确 3 个问题：

1. 哪些内核路径允许中断嵌套进入
2. 哪些临界区必须临时关中断
3. 中断进入后是否允许立即调度，还是只设置“需要调度”的标记

推荐的保守方案：

- 默认允许时钟中断打断普通内核执行
- 进入真正的共享数据临界区时，通过“关本核中断 + 局部锁”保护
- 中断处理里**不直接做复杂调度切换**，而是设置当前 hart 的 `need_resched`
- 在 trap 返回点或统一调度点检查 `need_resched`

这条路线比“在任意中断点立刻切线程”更稳，更适合教学内核逐步落地。

### 1.3 需要修改的模块

重点关注：

- `tg-rcore-tutorial-ch8/src/main.rs`
  - Trap 入口
  - timer interrupt 分发
  - syscall 返回路径
- `tg-rcore-tutorial-ch8/src/processor.rs`
  - 当前线程状态
  - 调度触发点
- `exp4-scheduler`
  - 若要更自然支持“时钟驱动的抢占/标记重调度”，可复用 `on_tick`
- `exp5-sync`
  - 把“中断上下文不能睡眠”“内核临界区如何保护”说清楚

### 1.4 正确性测试

建议至少做 3 个用户态测试：

1. `kernel_interrupt_timer`
   - 用户态频繁发起短 syscall
   - 内核在高频 syscall 往返中仍能持续收到 timer interrupt
   - 检查时钟计数单调增长、线程不会永久占住 CPU
2. `kernel_interrupt_long_syscall`
   - 人为构造一个较长的内核路径，例如大块 framebuffer 提交、反复文件读、或专门的测试 syscall
   - 测试长 syscall 期间 timer interrupt 仍会到达
3. `kernel_interrupt_preempt_flag`
   - 用户线程 A 不断陷入内核
   - 用户线程 B 观察自己是否能在合理时间内获得运行机会
   - 验证“内核态收到时钟后设置重调度标记”的路径有效

### 1.5 阶段验收

达到以下条件算完成：

- 不会因为内核态中断嵌套而死锁或破坏共享结构
- timer tick 在长 syscall 期间仍持续推进
- 内核日志能说明“中断已进入”“延迟调度已触发”

### 1.6 当前实现状态

当前仓库已经落了一个保守版 Phase 1，代码在 [`tg-rcore-tutorial-ch8/src/main.rs`](../tg-rcore-tutorial-ch8/src/main.rs)：

- 保留原来的“用户线程通过 `ForeignContext::execute()` 进入 U 态”主流程。
- 新增一个专门给 S 态内核代码使用的 `kernel_trap_entry`，只处理 `SupervisorTimer`。
- 中断上下文里只做 3 件事：
  - 统计 tick
  - 设置下一次 timer
  - 把 `need_resched` 置位
- 真正的线程切换仍然放在安全点做，也就是 trap 返回后的调度路径里。

这样做的原因是：

- 当前 `ch8` 还是教学内核，很多共享结构没有为“中断里直接切线程”准备好。
- 中断里只置 `need_resched`，可以把“内核态可响应中断”和“复杂调度切换”拆成两个阶段实现。
- 这也正好和 `exp4` 的职责边界一致：timer 先变成一次 tick，真正切换仍由调度层决定。

### 1.7 现在这版 Phase 1 的调用链

时钟中断的发送过程是：

1. 内核通过 `tg_sbi::set_timer(next_deadline)` 把“下一次触发时刻”写给 SBI。
2. 时间到达后，底层 timer 硬件触发 supervisor timer interrupt。
3. 如果此时 CPU 正在 U 态线程里运行，trap 会先回到 `ForeignContext/LocalContext` 的 trap 返回路径。
4. 如果此时 CPU 正在 S 态内核代码里运行，并且该段内核代码显式重新打开了中断，那么 trap 会进入 `kernel_trap_entry`。
5. 两条路径最终都会把 `need_resched` 置位，并在后续安全点检查它。

`need_resched` 的含义不是“立刻换掉当前线程”，而是：

- 当前 hart 在下一个安全点需要重新做一次调度决策。
- 最终结果可能是：
  - 当前线程继续跑
  - 当前线程被重新入队，换别的线程跑

所以它是“延期调度请求”，不是“已经完成切换”。

## Phase 2：为多核做结构改造

### 2.1 目标

先不急着把第二个 hart 真跑起来，而是先把单核假设拆掉，把“全局唯一处理器状态”改造成“每核一份局部状态 + 少量共享状态”。

### 2.2 重点改造点

需要重新审视以下结构是否含有“默认只有一个 CPU”的假设：

- 当前线程指针
- 调度器运行上下文
- trap 上下文保存区
- 定时器设置
- 中断使能状态
- 内核栈
- 日志与统计计数器

推荐拆分方式：

- 每个 hart 拥有自己的
  - `current_thread`
  - `idle_context`
  - `need_resched`
  - timer 事件与 tick 统计
  - 内核栈
- 全局共享的
  - 进程表 / 线程表
  - ready queue 或其分片
  - 同步原语对象
  - 文件系统和块设备

### 2.3 调度设计建议

T4 不必一开始就做复杂负载均衡。建议先做“能跑、能解释”的版本：

- 每核一个本地 run queue
- 新线程先放到 boot hart
- 空闲核可以从全局或其他核偷任务

保底版也可以是：

- 全局 ready queue + 每核 current thread

如果想降低首版难度，建议先用“全局 ready queue + 大粒度保护”做出正确性，再决定是否升级成“每核 run queue + work stealing”。

## Phase 3：真正启用多核

### 3.1 目标

在 QEMU 中启动多个 hart，让每个 hart 都完成：

- 基本初始化
- trap 向量安装
- 本核 timer 初始化
- 进入调度循环

### 3.2 推荐步骤

1. Boot hart 完成全局初始化
2. Boot hart 唤醒其他 hart
3. 次核进入统一的 `hart_main`
4. 每个 hart 完成本地初始化后进入 idle/schedule loop

需要特别关注：

- 次核不能重复初始化全局堆、全局设备、文件系统
- 次核必须有独立内核栈
- timer interrupt 要按 hart 分别设置
- trap handler 里要能知道“当前是哪个 hart”

### 3.3 锁与并发策略

多核下最容易出错的地方不是“启动”，而是“共享数据竞争”。

建议分 3 类处理：

1. 只能本核访问的数据
   - 不加全局锁，只做每核局部保护
2. 高频共享数据
   - 尽量细化锁粒度
3. 低频共享数据
   - 首版允许用简单全局锁，先保正确

要重点检查：

- 线程状态切换是否会被两个 hart 同时操作
- `re_enque` / `fetch` 是否线程安全
- 同步原语的等待队列是否会跨核唤醒
- 文件系统是否需要额外串行化

## Phase 4：把同步与调度机制调成“多核可用”

### 4.1 exp4 的角色

`exp4` 是调度策略层，不直接等于完整 task manager。

在多核阶段，它的任务是：

- 提供可插拔的调度策略
- 让“timer tick -> need_resched -> pick_next”这条链条更清晰

如果时间有限，T4 首版可继续使用 `exp4::DefaultTaskManager` 的默认 FIFO 行为，不必同步引入复杂调度算法。重点是先证明：

- 多核下调度框架仍正确
- 默认行为与单核逻辑一致

### 4.2 exp5 的角色

`exp5` 是同步原语层。

在多核阶段需要额外明确：

- 哪些锁允许在中断上下文使用
- 哪些锁只能在线程上下文使用
- 哪些路径必须改用 spin，而不能 sleep

建议约束：

- 中断处理程序里只允许用自旋型保护，不允许阻塞锁
- 线程/进程表、run queue、跨核唤醒列表这类核心共享结构优先用 spin 保护
- 面向用户态语义的阻塞锁仍可保留，但不能在中断路径里使用

## Phase 5：用户态正确性测试设计

测试不要只做“能跑通”，而要把 T4 两个核心目标拆开验证。

### 5.1 内核态中断响应测试

建议新增以下用户程序：

- `t4_irq_tick_progress`
  - 多次发起 syscall，观察时钟推进
- `t4_irq_long_kernel_path`
  - 触发长时间内核路径，检查中断计数是否增长
- `t4_irq_latency`
  - 用用户态测量“一个线程在另一个线程频繁陷入内核时”的最坏等待时间

关注点：

- tick 不丢
- 不死锁
- 不因中断重入破坏数据结构

### 5.2 多核正确性测试

建议新增以下用户程序：

- `t4_smp_parallel_hello`
  - 多线程打印 `tid + hartid`
  - 用来确认线程确实可分布到不同核
- `t4_smp_barrier`
  - 多线程通过 barrier 同步，验证所有线程都能推进
- `t4_smp_counter`
  - 多线程并发累加共享计数器
  - 分别测试无锁、mutex、rwlock 三种情形
- `t4_smp_pingpong`
  - 两组线程跨核互相唤醒，验证唤醒/调度路径稳定

### 5.3 回归测试

除了新增测试，还要回归：

- ch8 原有线程测试
- 原有 mutex/semaphore/condvar 测试
- Doom 是否仍能运行

## Phase 6：性能展示应用设计

T4 要求的是“相对复杂的用户态应用”，因此不建议只交一个共享计数器 benchmark。

推荐准备两层展示：

### 6.1 基础性能基准

用于清楚证明“真的变快了”：

- 多线程计数 / 归并 / 矩阵分块乘
- 单核 vs 双核/四核
- 统计总耗时、吞吐量、加速比

### 6.2 复杂应用

推荐两个候选方案，优先选第一个：

1. 多线程软件渲染程序
   - 例如 Mandelbrot、光线投射、分块 rasterizer、粒子系统
   - 每个线程负责一部分 framebuffer tile
   - 优点是并行度天然明确，图形效果直观，性能提升容易展示
2. 多线程版 Doom 扩展
   - 例如把帧渲染拆成多条 worker 线程处理列/块
   - 优点是和当前 `ch8 + doom` 路线最贴近
   - 缺点是改动大、调试难度高，不适合作为首个稳定里程碑

推荐的保底实现是：

- 新写一个“多线程 tile renderer”
- 由主线程发任务，worker 线程并行渲染不同屏幕块
- 最后主线程统一提交 framebuffer

这样既能体现：

- 多核让渲染变快
- 内核态中断响应保证系统在高负载渲染期间仍有时钟与输入响应

### 6.3 建议展示指标

- 1 核 / 2 核 / 4 核下的总渲染时间
- 平均每帧耗时
- FPS
- 输入到画面变化的延迟
- 高负载期间 shell 或后台线程的响应延迟

## 5. 推荐实现顺序

建议按下面顺序推进：

1. 冻结单核基线并补测量脚本
2. 做“内核态可响应 timer interrupt”的保守版本
3. 为处理器状态做 per-hart 拆分
4. 启动第二个 hart，先做最小并行运行
5. 把线程调度接到多核
6. 补多核下的同步保护与跨核唤醒
7. 增加正确性测试
8. 实现复杂性能展示应用
9. 最后再考虑进一步优化，如更细粒度调度、work stealing、Doom 并行渲染

## 6. 里程碑与判据

### M1：内核态中断响应完成

判据：

- 长 syscall 期间 tick 继续推进
- 不出现中断重入导致的崩溃
- 单核下原有 ch8 用例保持通过

### M2：双核最小可运行

判据：

- QEMU 可启动至少 2 个 hart
- 两个 hart 都能进入 trap 和调度路径
- 基础多线程测试可运行

### M3：多核同步正确

判据：

- 共享计数器在加锁版本下结果正确
- barrier、pingpong、condvar 场景稳定
- 不出现明显死锁和丢唤醒

### M4：复杂应用展示性能收益

判据：

- 复杂应用在多核下有稳定加速
- 有清晰的单核/双核/四核对比数据
- 能解释性能提升来自哪些机制

## 7. 风险点

最可能卡住的点有：

- 内核态允许中断后，原来默认“不会被打断”的代码路径暴露竞态
- 多核后全局静态状态需要改为 per-hart，本地/共享边界容易划错
- 同步原语在中断上下文与线程上下文的适用范围不同
- 文件系统、framebuffer、控制台输出这些共享设备路径容易成为瓶颈
- Doom 并行化本身难度较高，不适合作为唯一性能展示方案

因此建议：

- 复杂应用一定要准备保底方案，不把成败完全压在 Doom 并行化上
- 每做完一个阶段就留一个稳定 tag 或分支

## 8. 建议参考关系

可以把本项目已有实验当作分层参考：

- `ch3`：单核 timer interrupt 与抢占
- `ch5`：调度与进程管理
- `ch8`：线程与同步
- `exp4`：调度策略抽象
- `exp5`：同步原语与内核可移植实现

外部参考方面，可重点学习支持多核的内核项目在以下方面的做法：

- boot hart / secondary hart 启动顺序
- per-CPU 数据结构
- IPI 或跨核唤醒机制
- 中断上下文与线程上下文的锁分层

但实现时仍建议优先贴合当前教程代码结构，不要一次性搬入过重的框架。

## 8.1 关键概念解释

### boot hart / secondary hart 启动顺序是什么

在 RISC-V 多核系统里，hart 可以理解为“一个硬件线程”或“一颗逻辑 CPU”。其中：

- `boot hart` 是最先启动、负责全局初始化的那一核
- `secondary hart` 是后续被 boot hart 唤醒的其他核

“启动顺序”讨论的是：哪一核先做哪些事、哪些初始化只能做一次、次核什么时候才能进入调度。

一个常见顺序是：

1. boot hart 清空 BSS、初始化堆、页表、设备、全局任务系统
2. boot hart 为每个 secondary hart 准备启动栈与入口地址
3. boot hart 通过平台相关机制唤醒其他 hart
4. secondary hart 做“每核私有初始化”，例如本核 trap、timer、per-CPU 变量、idle task
5. 所有 hart 都准备好后再一起进入调度或空闲循环

这个顺序的核心目的是避免：

- 全局资源被重复初始化
- 次核在全局状态未就绪时提前访问共享数据
- 次核没有独立栈/本地状态就开始处理中断

### per-CPU 数据结构是什么

`per-CPU` 或 `per-hart` 数据结构，指的是“每个 CPU 都有自己独立一份”的数据，而不是所有 CPU 共用一份。

典型例子包括：

- 当前正在运行的线程指针
- 当前 CPU 的 idle 上下文/调度上下文
- 本核 `need_resched` 标记
- 本核 timer tick 计数
- 本核中断嵌套层数
- 本核内核栈
- 本核 run queue

这么做的目的主要有两点：

1. 降低共享竞争
   - 每个核频繁访问自己的本地数据，不必每次都抢全局锁
2. 保证语义正确
   - “当前线程”“当前 hart 是否需要调度”本来就是 CPU 局部概念，不适合做成全局唯一变量

### IPI 或跨核唤醒机制是什么

IPI 是 Inter-Processor Interrupt，也就是“处理器之间互相发中断”。

它的典型用途包括：

- 一个 CPU 想让另一个 CPU 立刻处理某件事
- 某个线程被唤醒后，目标 CPU 正在跑别的任务，需要提醒它重调度
- TLB shootdown、跨核回调、跨核停止点同步

“跨核唤醒机制”不一定非得是 IPI，但在 SMP 内核里，IPI 是最常见且最直接的一种做法。

常见流程是：

1. CPU A 把“待处理事件”写入 CPU B 的事件队列
2. CPU A 向 CPU B 发送 IPI
3. CPU B 进入 IPI handler
4. CPU B 从自己的 IPI 队列中取出事件并执行
5. 若需要，则设置 `need_resched` 或直接唤醒目标线程

### 中断上下文与线程上下文的锁分层是什么

“锁分层”指的是：不同执行上下文允许使用的锁不一样，不能混用。

这里至少要区分两种上下文：

- 线程上下文
  - 正在运行普通内核线程或 syscall 路径
  - 可以睡眠、可以被调度切走
- 中断上下文
  - 正在处理中断
  - 不能睡眠、不能去拿会阻塞的锁、不能做依赖当前线程存在的操作

所以通常会分层：

- 中断上下文只允许用 `spin lock` 或“关中断保护”
- 线程上下文可以使用阻塞型 mutex / semaphore / condvar

如果把会睡眠的锁拿到中断上下文里，很容易出错，因为中断处理程序没有“安全睡过去再等别人唤醒”的语义基础。

这也是为什么 T4 里，`exp5` 的 `SpinLock` 和 `MutexBlocking` 需要明确分工：

- `SpinLock` 更适合 run queue、当前线程指针、跨核事件队列这类核心短临界区
- `MutexBlocking` 适合线程上下文中的较长共享资源访问

## 8.2 ArceOS 是怎么实现这些机制的

下面的总结基于我本地拉下来的 `arceos` 仓库代码阅读结果。

### 1. boot hart / secondary hart 启动顺序

ArceOS 的启动顺序相当清晰，主入口在：

- `/tmp/arceos/modules/axruntime/src/lib.rs`
- `/tmp/arceos/modules/axruntime/src/mp.rs`

它的做法可以概括成：

1. primary CPU 进入 `rust_main(cpu_id, arg)`
2. 先做一次性的全局初始化
   - 清 BSS
   - `init_percpu(cpu_id)`
   - `init_early(cpu_id, arg)`
   - 内存管理、设备、调度器等初始化
3. 然后调用 `start_secondary_cpus(cpu_id)` 启动其他 CPU
4. 每个 secondary CPU 进入 `rust_main_secondary(cpu_id)`
5. secondary CPU 只做“本核初始化”
   - `init_percpu_secondary(cpu_id)`
   - `init_early_secondary(cpu_id)`
   - `init_memory_management_secondary()`
   - `init_later_secondary(cpu_id)`
   - `init_scheduler_secondary()`
6. 所有 CPU 都初始化完成后，再统一进入正常运行

比较值得参考的细节有两个：

- 它给 secondary CPU 单独准备了启动栈 `SECONDARY_BOOT_STACK`
- primary CPU 用 `ENTERED_CPUS` / `INITED_CPUS` 这两个原子计数来等次核完成进入与初始化，避免次核“半初始化”就参与系统运行

这对你的 T4 很有参考价值，因为你也需要把“全局只做一次”和“每核都要做一次”的初始化严格拆开。

### 2. per-CPU 数据结构

ArceOS 的 per-CPU 机制主要在：

- `/tmp/arceos/modules/axhal/src/percpu.rs`
- `/tmp/arceos/modules/axtask/src/run_queue.rs`

它大量使用 `#[percpu::def_percpu]` 声明每核独立变量，例如：

- `CPU_ID`
- `IS_BSP`
- `CURRENT_TASK_PTR`
- 每核 `RUN_QUEUE`
- 每核 `IDLE_TASK`
- 每核退出任务队列和等待队列
- 每核 IPI 事件队列

它的思路不是“所有东西都做成全局锁保护”，而是：

- 当前 CPU 自己频繁访问的数据，尽量做成 per-CPU
- 只有需要跨核共享的对象，才进入共享结构 + 锁保护

另外一个非常实用的点是：ArceOS 把“当前任务指针”也做成 per-CPU 数据，并且在某些架构上还做了寄存器缓存。这说明“current task”这种高频数据，确实适合优先走 per-CPU 设计。

### 3. IPI 或跨核唤醒机制

ArceOS 的 IPI 代码主要在：

- `/tmp/arceos/modules/axipi/src/lib.rs`
- `/tmp/arceos/modules/axhal/src/irq.rs`
- `/tmp/arceos/modules/axruntime/src/lib.rs`

它的机制不是“只发一个裸 IPI 就完了”，而是“IPI + 每核事件队列”的组合：

1. 每个 CPU 都有一个 per-CPU 的 `IPI_EVENT_QUEUE`
2. `run_on_cpu(dest_cpu, callback)` 会先把回调压入目标 CPU 的事件队列
3. 然后调用 `send_ipi(...)` 真正向目标 CPU 发送 IPI
4. 目标 CPU 进入 `ipi_handler()`
5. `ipi_handler()` 从本核事件队列中取出 callback 并执行

这个设计的优点是：

- IPI 本身只负责“把对方打断”
- 真正要做的事放在软件队列里，逻辑更灵活
- 后续可以扩展成跨核唤醒、TLB shootdown、跨核调度通知等多种用途

对你的 T4 来说，可以先不做通用 callback 框架，但可以借鉴这个思路实现一个简化版：

- 每核一个“待唤醒/待重调度”标志或事件槽
- 跨核唤醒时先写目标核队列
- 再发 IPI 提醒目标核尽快处理

### 4. 中断上下文与线程上下文的锁分层

这部分是 ArceOS 里最值得借鉴的地方之一，相关代码主要在：

- `/tmp/arceos/modules/axhal/src/irq.rs`
- `/tmp/arceos/modules/axtask/src/run_queue.rs`
- `/tmp/arceos/modules/axtask/src/wait_queue.rs`
- `/tmp/arceos/modules/axsync/src/mutex.rs`

它的分层思路大致是：

- 中断处理入口先用 `NoPreempt` 保护，保证处理中断时不会被调度打断
- 需要“关中断 + 禁止抢占”的调度关键路径，用 `NoPreemptIrqSave`
- wait queue 这种既会被线程阻塞路径访问、又涉及共享队列的数据结构，用 `SpinNoIrq`
- run queue 内部真正的 scheduler，用 `SpinRaw`
  - 因为外层已经持有 guard，IRQ 和 preempt 已经被控制住了，所以内层不再重复做更重的保护
- 面向普通线程的睡眠锁放在 `axsync::Mutex`
  - 它内部会通过 `WaitQueue` 让当前线程睡眠，因此本质上只能在线程上下文使用，不适合中断上下文

这里最关键的不是“锁名字要一样”，而是它体现了 3 个原则：

1. 外层已经禁止中断/抢占时，内层锁可以更轻
2. 中断路径和线程阻塞路径必须分开
3. run queue、wait queue、task state 这些核心结构的状态转换必须非常明确

对你的 T4 来说，可以直接借鉴成下面这条规则：

- trap / timer / IPI handler 中只允许使用“不会睡眠”的保护手段
- syscall / 线程路径里才允许进入阻塞型同步原语
- 调度器和等待队列要明确谁负责“关中断”，谁只是“纯数据结构自旋锁”

## 8.3 对当前项目的直接启发

把上面这些经验映射到你现在的 `tg-rcore-tutorial-ch8 + exp4 + exp5`，可以得到一个比较务实的落地方案：

1. 启动阶段
   - `boot hart` 负责一次性的内存、设备、用户程序镜像、全局线程表初始化
   - `secondary hart` 只做本核 trap、timer、current thread、idle context 初始化
2. per-hart 数据
   - 先把“当前线程”“need_resched”“内核栈”“idle 上下文”改成每核一份
3. 跨核唤醒
   - 首版可以先做“目标核 `need_resched` + IPI”
   - 再逐步升级成“每核事件队列 + IPI”
4. 锁分层
   - `exp5::SpinLock` 用于中断路径和核心调度结构
   - `exp5::MutexBlocking / Semaphore / Condvar` 只放在线程上下文
5. 调度路径
   - `exp4` 负责把 timer tick 变成 `on_tick` / `pick_next` 的调度决策
   - 真正的中断进入、返回点、是否立即切换线程，仍由 `ch8` 的 trap 与 processor 路径控制

## 9. 建议的文档产出

T4 最终建议至少补 3 份文档：

1. `docs/t4-plan.md`
   - 现在这份计划
2. `docs/t4-design.md`
   - 内核态中断响应机制
   - 多核启动与 per-hart 设计
3. `docs/t4-results.md`
   - 正确性测试结果
   - 性能测试数据
   - 与单核基线对比分析

## 10. 一句话执行建议

主线建议是：**以当前 `ch8 + doom` 为主体，先做“单核下内核态可安全响应时钟中断”，再做 per-hart 拆分与双核启动，最后用多线程图形渲染程序展示性能提升；Doom 并行化作为进阶项而不是首个里程碑。**
