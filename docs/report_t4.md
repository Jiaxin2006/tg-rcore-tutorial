# T4 Report

## 1. 目标与完成情况

本次 T4 的目标有四项：

1. 支持内核态响应中断
2. 支持多核处理
3. 设计用户态测试程序验证正确性
4. 设计较复杂的用户态应用展示多核带来的性能变化

当前仓库已经完成这四项交付：

- 内核态响应中断：已完成，当前支持在 S 态较长内核路径中响应 `SupervisorTimer`
- 多核处理：已完成，当前支持 2 hart 启动、per-hart 本地状态、共享调度器、多线程跨 hart 运行
- 正确性测试：已完成，提供 `kernel_interrupt_check` 和 `kernel_smp_check`
- 性能展示：已完成，提供 `smp_bench`

当前实现的边界也需要明确：

- 这版内核 trap 入口只支持 `SupervisorTimer`，其他 kernel interrupt / exception 仍会 panic
- 多核版先采用保守的 `KERNEL_BIG_LOCK`，优先保证正确性，而不是追求极致并行
- Doom 已经能跑在多核内核上，但 Doom 本身仍是单线程程序，并不会并行渲染

## 2. 基线与测量口径

原先 `t4-plan.md` 和 `t4-phase0-baseline.md` 中关于基线的核心内容已经并入本报告。

后续所有 T4 对比默认采用下面的统一口径：

- 内核：`tg-rcore-tutorial-ch8`
- 构建模式：`debug`
- 用户态镜像：由 `build.rs` 自动打包
- 默认 `cargo run` 机器配置：`-smp 2`
- 若要显式比较单核 / 双核：使用 `scripts/run-ch8-qemu.sh --smp 1|2`

建议固定的测量对象包括：

- `kernel_interrupt_check`：验证内核态 timer interrupt
- `kernel_smp_check`：验证多核 bring-up、per-hart timer、用户线程跨 hart 运行
- `smp_bench`：验证复杂用户态应用在 1 worker / N workers 下的性能差异
- `doomgeneric`：验证图形、文件系统、终端输入接入

## 3. 内核态中断的原理

### 3.1 核心思路

这版实现没有选择“中断一到就立刻在内核里切线程”，而是采用更保守的两段式设计：

1. 允许 S 态内核代码被 timer interrupt 打断
2. 中断里只做最小工作：
   - 记录 tick
   - 设置下一次 timer
   - 置位当前 hart 的 `need_resched`
3. 真正的调度切换延后到安全点处理

这样做的好处是：

- 先把“内核态也能响应中断”做正确
- 避免在嵌套 trap 里直接修改复杂共享状态
- 与当前教学内核的结构更匹配

### 3.2 实现路径

关键实现位于 `tg-rcore-tutorial-ch8/src/main.rs`：

- `KernelInterruptGuard`
  - 在较长的内核路径里临时安装 `kernel_trap_entry`
  - 打开 `sstatus.sie`
  - 允许 timer interrupt 打断当前 S 态代码
- `kernel_trap_entry`
  - 保存寄存器现场
  - 跳到 `kernel_trap_handler`
- `kernel_trap_handler`
  - 当前只处理 `SupervisorTimer`
  - 调用 `record_timer_tick()`
  - `record_timer_tick()` 会更新本 hart 的 `timer_ticks`
  - 同时置位 `need_resched`
  - 并通过 `program_next_timer()` 重新编程下一次 timer

M 态到 S 态的 timer 转发也已经补齐：

- `tg-rcore-tutorial-sbi` 负责设置 `mtimecmp`
- M 态 timer 到达后显式置位 `STIP`
- S 态最终收到 `SupervisorTimer`

### 3.3 为什么当前只处理中断，不直接在 trap 里切换

因为当前内核的共享状态仍较多：

- 调度器
- 线程状态
- 进程对象
- 文件描述符表
- 同步原语等待队列

如果在 kernel trap 内直接抢占并切线程，复杂度会显著上升。当前的 `need_resched` 方案本质上是“延期调度请求”，中断只负责发出请求，真正切换放在统一调度点完成。

## 4. 多核的实现方式

### 4.1 boot hart / secondary hart

当前多核启动采用“boot hart 一次性初始化 + secondary hart 本地 bring-up”的结构：

- `hart0`
  - 清 BSS
  - 初始化堆、页表、portal、syscall、设备
  - 加载第一个用户程序
  - 放行次核
- `hart1`
  - 不重复做全局初始化
  - 只激活内核地址空间
  - 安装本核 trap
  - 设置本核 timer
  - 进入同一套 `scheduler_loop`

内核里用这些量跟踪多核状态：

- `ONLINE_HARTS`
- `ONLINE_HART_MASK`
- `current_hart_id()`

### 4.2 per-hart 本地状态

当前已经拆成 per-hart 的状态包括：

- `need_resched`
- `timer_ticks`
- `kernel_timer_interrupts`
- 当前运行线程 `current`
- portal slot

实现上主要对应：

- `HART_LOCAL`
- `PThreadManager.current: [Option<ThreadId>; MAX_HARTS]`
- `MultislotPortal::init_transit(..., MAX_HARTS)`

这样每个 hart 都有自己的一份“当前 CPU 局部状态”，不再沿用单核时代“全局只有一个 current”的假设。

### 4.3 当前多核调度策略

当前调度器采用：

- 每个 hart 一条本地 ready queue
- 本地队列优先
- 本地为空时从其他 hart 的队列尾部偷任务

具体策略是：

1. `fetch_for(hart_id)` 先从本地队列 `pop_front()`
2. 本地没有任务时，选择 ready queue 最长的其他 hart 作为 victim
3. 从 victim 队列 `pop_back()` 实现 work stealing

线程运行状态采用三态：

- `Runnable`
- `Running(hart_id)`
- `Blocked`

这样可以避免同一个线程同时被两个 hart 选中。

### 4.4 当前多核“开关”在哪里

当前没有内核内部的“单核 / 多核运行时开关”，控制点在 QEMU 层：

- `cargo run` 默认使用 `tg-rcore-tutorial-ch8/.cargo/config.toml` 里的 `-smp 2`
- 如果要显式切到单核 / 双核，可以使用新增脚本：

```bash
bash scripts/run-ch8-qemu.sh --smp 1
bash scripts/run-ch8-qemu.sh --smp 2
```

推荐的使用方式是：

```bash
cd tg-rcore-tutorial-ch8
TG_CH8_INIT_APP=smp_bench cargo build --features exercise
bash ../scripts/run-ch8-qemu.sh --smp 1
bash ../scripts/run-ch8-qemu.sh --smp 2
```

也就是说，“多核开关”目前本质上是 QEMU 的 `-smp N`。

## 5. 锁与并发策略

### 5.1 第一层：大内核锁

当前 T4 采用的是保守的第一版 SMP 保护策略：

- `KERNEL_BIG_LOCK`

它的使用原则是：

- 进入共享内核路径前先拿锁
- 切到用户态执行前释放锁
- 用户态 trap 回内核后重新拿锁

因此当前的并行性是：

- 多个 hart 可以并行执行用户线程
- 共享内核状态仍然一次只允许一个 hart 修改

这是一个典型的“先保证正确，再逐步细化锁粒度”的教学实现。

### 5.2 第二层：对象内部状态保护

同步原语本身仍然维护自己的内部状态，例如：

- `MutexBlocking`
- `Semaphore`
- `Condvar`
- `RwLock`

这些对象内部会维护：

- `holder`
- `wait_queue`
- 资源计数

当前 `exp5-sync` 的 kernel 实现大量使用 `UPIntrFreeCell`。这一点非常重要：

- `UPIntrFreeCell` 的语义是“关本核中断 + 保护当前 CPU 上的临界区”
- 它本身不是一个完整的跨 hart SMP 锁

所以当前 T4 的正确性依赖关系是：

1. 跨 hart 的共享内核路径先由 `KERNEL_BIG_LOCK` 串行化
2. 对象内部再用 `UPIntrFreeCell` 维护本对象状态的一致性

这也是为什么现在说“多核策略是保守的”：

- 先用一把大锁覆盖共享内核路径
- 而不是立刻把所有子系统都改造成细粒度 SMP 锁

### 5.3 什么时候阻塞，什么时候自旋

这版的判断标准是“同步原语语义”决定调度动作：

- `MutexBlocking` / `Semaphore` / `Condvar` / `RwLock`
  - 获取失败后返回 `-1`
  - 调度器把当前线程标记为 `Blocked`
- `SpinLock`
  - 获取失败后不进入阻塞队列语义
  - 返回忙失败
  - 调度器不会把它当作真正的睡眠阻塞

因此当前“锁和并发策略”的判断逻辑可以概括为：

- CPU 局部状态：直接 per-hart 化
- 共享内核路径：先走 `KERNEL_BIG_LOCK`
- 同步对象内部：对象自己维护等待队列与持有者
- 是否阻塞：由 syscall 返回码和同步原语语义决定

## 6. 测试方式

### 6.1 内核态中断测试：`kernel_interrupt_check`

用途：

- 验证一次长 syscall 中，内核是否真的收到了 timer interrupt

方法：

1. 用户态调用 `kernel_interrupt_check(min_interrupts)`
2. 内核进入较长路径
3. 通过 `KernelInterruptGuard` 打开 S 态中断
4. 内核忙等一段时间
5. 统计 `kernel_timer_interrupts` 是否增长到目标值

通过条件：

- 在一次长 syscall 中观测到至少 2 次内核态 timer interrupt

命令：

```bash
cd tg-rcore-tutorial-ch8
TG_CH8_INIT_APP=kernel_interrupt_check cargo run --features exercise
```

### 6.2 多核正确性测试：`kernel_smp_check`

用途：

- 验证两个 hart 都已上线
- 验证两个 hart 的本地 timer 都在前进
- 验证用户线程集合确实跑到了多个 hart 上

方法分两段：

1. 先做两次 `kernel_hart_snapshot`
   - 检查 `online_mask`
   - 检查 `timer_ticks` 增量
2. 再创建多个用户线程
   - 每个线程反复调用 `kernel_hart_snapshot`
   - 汇总 `user observed hart mask`

通过条件：

- `online_mask` 包含 `hart0` 和 `hart1`
- 两个 hart 的 `timer_ticks` 都增长
- `user observed hart mask=0x3`

命令：

```bash
cd tg-rcore-tutorial-ch8
TG_CH8_INIT_APP=kernel_smp_check cargo run --features exercise
```

### 6.3 性能展示程序：`smp_bench`

#### 逻辑

`smp_bench` 现在是一个确定性的分块渲染 benchmark：

1. 把 640x400 的 framebuffer 切成 320 个 tile
2. 每个 tile 做固定点 Mandelbrot 风格计算
3. 比较两种模式：
   - `1 worker`
   - `online_harts workers`
4. 主线程自己也参与计算，作为 `worker 0`
5. 额外线程作为 `worker 1..N-1`
6. 所有 worker 都做同一份总工作量，只是按 `tile_id % worker_count` 分工
7. 记录：
   - 每轮耗时
   - 校验和
   - `observed_harts`

这版 benchmark 有两个关键点：

- 对比的是同一 guest 内的 `1 worker` 和 `N workers`，不是只看“开了 2 hart 就一定变快”
- 会验证 checksum 一致，避免“少算了工作”带来虚假的提速

#### 当前实测结果

当前仓库里我实际跑到的结果如下：

- `--smp 2`
  - `1 worker avg = 1,579,220 us`
  - `2 workers avg = 857,663 us`
  - `speedup = 1.84x`
- `--smp 1`
  - `1 worker avg = 1,112,014 us`
  - `1 worker avg = 1,112,335 us`
  - `speedup = 0.99x`

这说明：

- 在双核 guest 里，`smp_bench` 已经能展示明显的并行收益
- 在单核 guest 里，它不会凭空制造“多核加速”

需要注意一个细节：

- “单核 guest 的 1 worker” 和 “双核 guest 的 1 worker” 不一定完全等价
- 双核 guest 会多一个次核和本地 timer，因此额外开销略有不同

因此最核心的性能结论应看：

- 同一双核 guest 里 `1 worker -> 2 workers` 的速度提升

#### 运行命令

```bash
cd tg-rcore-tutorial-ch8
TG_CH8_INIT_APP=smp_bench cargo build --features exercise
bash ../scripts/run-ch8-qemu.sh --smp 1
bash ../scripts/run-ch8-qemu.sh --smp 2
```

## 7. Doom 当前的接入方式

### 7.1 Doom 是否支持多核

当前答案是：

- Doom 运行在支持 SMP 的内核上：是
- Doom 本身已经改造成多线程并行应用：否

原因很简单：

- 当前 `doomgeneric` 仍然只有一个用户线程
- 它可以被调度到不同 hart
- 但不会同时在两个 hart 上并行渲染或并行更新游戏逻辑

所以 Doom 现在更适合作为：

- 图形路径
- 文件系统路径
- 键盘输入路径

的综合验证，而不是“多核性能展示应用”。

### 7.2 Doom 的资源和设备接入

当前 Doom 接入依赖四条路径：

1. 文件系统
   - `doomgeneric` ELF 和 `doom1.wad` 都被打进 `fs.img`
2. 块设备
   - QEMU `virtio-blk-device`
3. 图形
   - QEMU `virtio-gpu-device`
4. 键盘输入
   - QEMU `-serial mon:stdio`

因此 Doom 的键盘焦点不在图形窗口，而在启动 `cargo run` 或 QEMU 的那个终端。

### 7.3 键盘是怎么接入的

当前键盘路径是：

1. QEMU 把终端输入送到串口
2. 内核 `input_getchar()` 直接读 UART MMIO：
   - `UART_LSR`
   - `UART_RBR`
3. Doom 平台层 `poll_input()` 轮询 `SYS_input_getchar`
4. `DG_GetKey()` 从本地队列取键
5. `i_input.c` 里的 `I_GetEvent()` 把它转成：
   - `ev_keydown`
   - `ev_keyup`
6. Doom 最终通过 `D_PostEvent()` 接收按键事件

当前已经接上的常用按键包括：

- `WASD` / 方向键
- `Q`
- `Enter`
- `J`
- `K`
- `U`

### 7.4 还没有接入的输入

当前还没有真正接入的是：

- 鼠标
- 声音 / 音乐

其中声音在启动参数里已经显式关闭：

- interactive：`-nomusic -nosound`

鼠标方面，Doom 本身是支持 `ev_mouse` 事件的，但当前平台层还没有提供鼠标输入源。

### 7.5 当前交互 / demo 模式

当前 `Makefile.rcore` 默认是：

- `DG_MODE ?= interactive`

其中：

- `interactive`
  - `-iwad doom1.wad -nomusic -nosound`
- `demo`
  - `-iwad doom1.wad -playdemo demo1`

切换命令：

```bash
cd tg-rcore-tutorial-ch8/doomgeneric/doomgeneric
make -f Makefile.rcore DG_MODE=interactive
make -f Makefile.rcore DG_MODE=demo
```

然后回到 `tg-rcore-tutorial-ch8` 重新 `cargo build` / `cargo run`，把新的 Doom ELF 重新打进 `fs.img`。

## 8. 结论

当前 T4 可以总结为：

1. 内核态响应中断已经完成，原理是“允许 S 态被 timer 打断，但调度延后到安全点”
2. 多核已经完成，核心做法是“boot/secondary 分流 + per-hart 本地状态 + 本地队列 + work stealing”
3. 正确性测试已经补齐：
   - `kernel_interrupt_check`
   - `kernel_smp_check`
4. 复杂性能展示程序已经补齐：
   - `smp_bench`
   - 并且当前实测能显示双核 guest 中 `1.84x` 的提速
5. Doom 已成功接入图形、文件系统和终端键盘输入，但仍是单线程程序

这版实现的技术取向是：

- 先用 `KERNEL_BIG_LOCK` 保守落地 SMP 正确性
- 先证明“内核态中断”和“多核线程调度”都能工作
- 再在此基础上展示复杂用户态程序的性能收益
