# T4 Report

## 内核态响应中断

### 实现方式
1. 在 [`tg-rcore-tutorial-ch8/src/main.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/src/main.rs) 中新增了一套给 **S 态内核代码** 使用的 trap 入口：
   - `kernel_trap_entry`：保存当前内核寄存器现场后跳到 Rust handler
   - `kernel_trap_handler`：当前只处理 `SupervisorTimer`
   - `record_timer_tick` / `NEED_RESCHED` / `KERNEL_TIMER_INTERRUPTS`：记录 tick、置位延迟调度标志、统计内核态 timer interrupt 次数
2. 这点在当前共享调度器版本里**仍然如此**：`kernel_trap_handler` 现在依旧只支持 `SupervisorTimer`。除了 `SupervisorTimer` 之外的其他 **kernel trap**（包括异常和其他中断）目前都会 panic。
3. 在 [`syscall-t3l8/src/kernel/mod.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/syscall-t3l8/src/kernel/mod.rs) 中新增 `KernelTest` trait 和 `KERNEL_INTERRUPT_CHECK` syscall 分发，用来把这项能力暴露给用户态测试程序。
4. 在 [`tg-rcore-tutorial-sbi/src/msbi.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-sbi/src/msbi.rs) 和 [`tg-rcore-tutorial-sbi/src/m_entry.asm`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-sbi/src/m_entry.asm) 中补齐了最小 SBI 的 timer 转发逻辑：
   - `set_timer` 写入 `mtimecmp` 后重新打开 `MTIE`
   - M 态收到 `machine timer interrupt` 后显式置位 `STIP`
   - M 态 trap 返回时只对 `ecall` 平移 `mepc`，不会把 timer interrupt 当成 `ecall` 处理

### 测试
1. 在 [`tg-rcore-tutorial-ch8/src/main.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/src/main.rs) 中实现 `kernel_interrupt_check` 这一条专用 syscall：
   - 进入内核后通过 `KernelInterruptGuard` 打开 S 态中断
   - 调用 `set_stimer()` 允许响应 S 态时钟中断
   - 调用 `program_next_timer()` 设定下一次 timer deadline
   - 在内核里忙等一段时间，统计 `KERNEL_TIMER_INTERRUPTS` 是否增长
2. 用户态测试程序位于 [`tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/kernel_interrupt_check.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/kernel_interrupt_check.rs)。
   它会调用上述 syscall，并以“在一次长 syscall 中观测到至少 2 次内核态 timer interrupt”为通过条件。
3. 这项测试也已经接入 [`ch8_usertest`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/ch8_usertest.rs) 的 exercise 用户测试集；若它退出码非 0，则 `ch8_usertest` 会直接失败。
4. 若要**单独运行**这条内核中断测试程序，可在 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8) 目录下执行：
   - `TG_CH8_INIT_APP=kernel_interrupt_check cargo run --features exercise`
5. 当前 timer 周期由 [`tg-rcore-tutorial-ch8/src/main.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/src/main.rs) 中的 `TIMER_INTERVAL = 62_500` 决定。
   - 结合 `clock_gettime` 中 `time * 10000 / 125` 的换算，可知这里默认假定 RISC-V `time` 频率约为 `12.5 MHz`
   - 因此 `62_500` 个 tick 大约对应 `5 ms`
   - 这比最初的 `1 ms` 更保守一些，能明显减少大应用启动期的抢占抖动；此前暴露出来的上下文损坏问题根因仍然是 `m_trap_vector` 返回路径没有恢复被异步中断打断时的 `a0/a1`，而不是“时间片本身就错误”

## 多核实现验证

### 当前状态
1. 当前内核已经完成：
   - `boot hart` / `secondary hart` 分流启动
   - 每个 hart 独立开启本地 `stimer`
   - `need_resched`、`timer_ticks`、`kernel_timer_interrupts`、以及 `PROCESSOR.current` 都改成 per-hart 状态
   - `thread_manager` 的相关操作 都已经改成 per-hart 状态
2. 这一阶段主要验证的是：
   - 两个 hart 都确实进入了内核
   - 两个 hart 都在独立接收本地 timer interrupt
   - “当前线程”这一类 CPU 局部状态不再是假定全局唯一

### 验证方式
1. 在 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8) 目录下单独运行多核检查程序：
   - `TG_CH8_INIT_APP=kernel_smp_check cargo run --features exercise`
2. 该程序会通过 `KERNEL_HART_SNAPSHOT` syscall 取两次内核快照，中间 `sleep(100)`，然后比较两次结果。
3. 观察点包括：
   - `online_mask=0x3`：表示 `hart0` 和 `hart1` 都已经 online
   - `hart0 delta: ticks=...`
   - `hart1 delta: ticks=... kernel_timer_interrupts=...`
   - `kernel smp check passed!`
4. 其中：
   - `delta` 表示第二次快照减去第一次快照，也就是这 100ms 窗口内的增量
   - `ticks` 表示对应 hart 的本地 timer 逻辑计数是否继续前进
   - `kernel_timer_interrupts` 只统计“该 hart 在 S 态内核代码执行期间收到 timer interrupt”的次数
5. 因此在当前实现中，看到 `hart1` 的 `kernel_timer_interrupts > 0` 而 `hart0` 的该项可能为 `0` 是正常的：
   - `hart1` 当前主要停在内核 idle loop 中，timer 到来时走的是 `kernel_trap_handler`
   - `hart0` 在 `kernel_smp_check` 期间大部分时间运行用户态测试程序，因此更多表现为“timer tick 在走”，而不是“内核态长路径里又被 timer 打断”
6. 多核回归时还可以再运行：
   - `TG_CH8_INIT_APP=kernel_interrupt_check cargo run --features exercise`
   - `TG_CH8_INIT_APP=ch8_usertest cargo run --features exercise`

## 共享调度器与 SMP 保护

### 当前实现
1. 当前已经从“次核只跑 idle + 本地 timer”的版本，推进到“两个 hart 共用同一个 ready queue”的第一版共享调度器。
2. 这版共享调度器的核心改动有 3 个：
   - `PROCESSOR.current` 改成 per-hart，每个 hart 维护自己的 current 线程
   - `MultislotPortal` 从 `1` 个 slot 扩展到按 `MAX_HARTS` 分 slot，并以 `hart_id` 作为 slot key，避免两个 hart 同时切用户态时覆盖同一块 portal cache
   - 在线程管理器里新增 `Runnable / Running(hart_id) / Blocked` 状态，避免共享 ready queue 中的同一个 TID 被重复选中
3. 另外，这一版用了一个**保守但有效**的 SMP 保护方案：`KERNEL_BIG_LOCK`。
4. 当前**还没有**实现“每个 hart 一条独立 ready queue”的分核调度队列；现在仍然是：
   - 一个全局共享 ready queue
   - 每个 hart 一份 `current`
   - 共享内核路径由 `KERNEL_BIG_LOCK` 串行化

### 什么是 SMP 保护
1. `SMP` 指的是多个 hart/CPU 会**同时**执行内核或用户线程。
2. 所谓 `SMP 保护`，本质上就是保证这些共享状态不会被多个 hart 同时改坏，例如：
   - ready queue
   - `PROCESSOR.current`
   - 进程地址空间 / 文件描述符表 / 信号状态
   - 锁、信号量、条件变量等待队列
3. 这次采用的是“大内核锁（Big Kernel Lock）”做法：
   - 进入共享调度器、syscall、阻塞/唤醒、进程线程管理这些**共享内核路径**之前，先拿 `KERNEL_BIG_LOCK`
   - 真正切到用户态执行之前释放这把锁
   - 从用户态 trap 回内核后，先重新拿锁，再处理 syscall / timer / 调度决策
4. 这样做的效果是：
   - 多个 hart 仍然可以**并行执行用户线程**
   - 但共享内核状态一次只允许一个 hart 修改，先保证正确性
5. 这里的“大内核锁”并不是“把共享变量变成各核互不共享”，而是“这些变量仍然共享，只是内核态访问时先串行化”。
6. 因此它能提供的是一种**保守的一致性保证**：
   - 如果所有共享内核状态都只在持锁路径里访问，那么这版实现就更容易维持正确语义
   - 但它不是自动正确，后续仍然要继续审计是否存在绕过大锁的共享状态访问
7. 它的边界也很明确：
   - 这是共享调度器的第一版，不是最终的细粒度并行内核
   - 后续如果想进一步提升内核并行度，还要把大锁逐步拆成更细的锁，或者改成更细粒度的 per-subsystem 保护

### 共享调度器验证
1. 当前的 [`kernel_smp_check`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/kernel_smp_check.rs) 已经扩展为两段验证：
   - 第一段仍然验证两个 hart 的本地 timer / kernel timer interrupt 是否都在前进
   - 第二段会创建多个用户线程，反复调用 `KERNEL_HART_SNAPSHOT`，统计用户线程实际跑过哪些 hart
2. 单独运行命令：
   - `TG_CH8_INIT_APP=kernel_smp_check cargo run --features exercise`
3. 通过现象包括：
   - `online_mask=0x3`
   - `hart0 delta: ticks=...`
   - `hart1 delta: ticks=... kernel_timer_interrupts=...`
   - `user observed hart mask=0x3`
   - `kernel smp check passed!`
4. 其中各条日志的含义并不相同：
   - `online_mask=0x3` 只说明 `hart0` 和 `hart1` 都已经上线
   - `hartX delta: ticks=...` 说明对应 hart 的本地 timer 仍在前进
   - `user observed hart mask=0x3` 才能说明用户线程实际已经在 `hart0` 和 `hart1` 上都运行过，不再只是“次核本地 timer 在走”
5. 如果打开调度日志，看到类似 `schedule: hart1 tid=ThreadId(...)` 也能作为旁证，说明 `hart1` 已经实际挑中了某个线程并执行。
6. 进一步的综合回归验证方式：
   - `TG_CH8_INIT_APP=ch8_usertest cargo run --features exercise`
   - `tg-rcore-tutorial-checker --ch 8 --exercise < /tmp/ch8_usertest_shared_sched_v2.log`
7. 这轮实测结果是：
   - `kernel_smp_check` 通过，日志中有 `user observed hart mask=0x3`
   - `kernel_interrupt_check` 仍能看到 `kernel_interrupt_check: hart0 success observed=2`
   - `ch8_usertest` 在共享调度器下完整通过，checker 结果为 `PASS 25/25`

## Doom 游戏：多核与交互模式

### 当前结论
1. 当前 `doomgeneric` 是一个**单线程用户态程序**，因此它**不会像 `kernel_smp_check` 那样同时在两个 hart 上并行执行多个游戏线程**。
2. 但它已经运行在支持 SMP 的 ch8 内核之上，因此：
   - 该游戏线程可以被调度到 `hart0` 或 `hart1`
   - 内核的另一个 hart 也仍然会继续处理本地 timer、中断与其他线程
3. 因而这里要区分两件事：
   - “游戏运行在多核内核上”：是
   - “游戏自身已经改造成并行多线程、同时利用多个 hart 渲染或更新逻辑”：否

### 游戏逻辑
1. 当前平台层入口位于 [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/doomgeneric_rcore.c`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/doomgeneric_rcore.c)。
2. 它的主逻辑与标准 `doomgeneric` 端口一致：
   - 先调用 `doomgeneric_Create(...)` 完成 Doom 初始化
   - 再在无限循环中持续调用 `doomgeneric_Tick()`
3. 当前平台层里传给 Doom 的启动参数分成两套：
   - `interactive`：`-iwad doom1.wad -nomusic -nosound`
   - `demo`：`-iwad doom1.wad -playdemo demo1`
4. 因此当前 `interactive` 模式已经不再强制 `warp` 进 `E1M1`，而是回到更接近老师演示的 title / attract / menu 流程。
5. 如果用户启动后**长时间不按键**，后续仍可能看到类似自动演示的画面，这属于 Doom 本身的 title / credits / attract mode 轮播，而不是本移植额外强制传入了 `-playdemo`。

### 外设接入
1. 当前这套 Doom 运行环境依赖的外设，都是通过 [`tg-rcore-tutorial-ch8/.cargo/config.toml`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/.cargo/config.toml) 里的 QEMU runner 参数接入的。
2. 块设备接入方式：
   - `-drive file=target/riscv64gc-unknown-none-elf/debug/fs.img,if=none,format=raw,id=x0`
   - `-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0`
   - 其中 `fs.img` 里包含内核测试程序、`doomgeneric` 用户程序以及 `doom1.wad`
3. 图形设备接入方式：
   - `-device virtio-gpu-device,bus=virtio-mmio-bus.1`
   - 默认显示后端是 `-display cocoa`；若宿主机不是 macOS，可按需要改成 `sdl` 或 `gtk`
4. 终端 / 键盘输入接入方式：
   - `-serial mon:stdio`
   - 因而键盘输入焦点在运行 `cargo run` 的终端，而不是 QEMU 图形窗口
5. Doom 资源接入方式：
   - 将 `doom1.wad` 放到 [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric)
   - 在该目录重新执行 `make -f Makefile.rcore`
   - 然后回到 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8) 再执行 `cargo build` 或 `cargo run`，把新的用户态 ELF 和 `doom1.wad` 一起重新打进 `fs.img`

### 输入支持
1. 当前默认构建模式已经改成 `interactive`：
   - [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/Makefile.rcore`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/Makefile.rcore) 中 `DG_MODE ?= interactive`
   - [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/doomgeneric_rcore.c`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/doomgeneric_rcore.c) 中 `DG_RCORE_INTERACTIVE` 默认也为 `1`
2. 在该模式下：
   - `DG_GetKey()` 会轮询 `input_getchar` syscall
   - 终端输入会映射到 Doom 按键，例如 `WASD`、方向键、`J`、`K`、`Q`
   - 键盘输入焦点在运行 `cargo run` 的终端，而不是 QEMU 图形窗口
3. 当前已经接上的交互 / 外设能力是：
   - 显示：VirtIO-GPU
   - 存储：VirtIO block（`fs.img`）
   - 键盘：串口终端输入，经 `input_getchar` 转成 Doom 按键
4. 当前**没有**接上的外设主要是：
   - 鼠标
   - 声音 / 音乐（`interactive` 启动参数里显式传了 `-nomusic -nosound`）
5. 当前可直接使用的常用按键为：
   - `Q`：打开 / 关闭 Doom 菜单
   - `WASD` 或方向键：移动 / 菜单导航
   - `Enter`：确认
   - `J`：开火
   - `K` 或空格：使用 / 开门
   - `U`：Run 修饰键
6. 因而如果“能看到画面，但按键没有反应”，首先要检查的是**终端焦点**，而不是 QEMU 图形窗口焦点。
7. 若要显式切换模式，可执行：
   - `cd tg-rcore-tutorial-ch8/doomgeneric/doomgeneric`
   - 交互版：`make -f Makefile.rcore DG_MODE=interactive`
   - demo 版：`make -f Makefile.rcore DG_MODE=demo`
   - 然后回到 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8) 执行 `cargo build` 或 `cargo run`，重新打包 `fs.img`
8. 若想看到老师演示里那种“可选 New Game / Options / Load Game”的界面，应使用 `interactive` 模式并重新打包；此时启动后按 `Q` 就可以主动拉起菜单。

### 如何验证
1. 重新构建并运行 Doom：
   - `cd tg-rcore-tutorial-ch8/doomgeneric/doomgeneric && make -f Makefile.rcore`
   - `cd ../../ && cargo run`
2. 当前实测启动日志中可以看到：
   - `[doomgeneric] online_harts=2 current_hart=1 online_mask=0x3`
   - `[doomgeneric] mode=interactive (keyboard from cargo run terminal)`
3. 这些日志说明：
   - Doom 运行时看到的确实是一个 `2 hart` 的内核环境
   - 当前启动的是交互版，而不是强制 `-playdemo demo1` 的 demo 版
4. 但仅靠 Doom 启动日志，还**不能**说明“游戏本身并行用到了两个 hart”，因为它仍然只有一个用户线程。
5. 若要验证“两个 hart 都确实在并行调度用户线程”，应运行：
   - `TG_CH8_INIT_APP=kernel_smp_check cargo run --features exercise`
6. 其中 `user observed hart mask=0x3` 才能说明用户线程集合已经分别在 `hart0` 和 `hart1` 上运行过；这项结论适用于当前共享调度器实现，而不是 Doom 自己内部做了多线程并行。

## 完整测试操作流程

### 0. 环境准备
1. 准备 RISC-V 交叉编译器，例如 `riscv64-elf-gcc`；若工具链前缀不同，先设置：
   - `export RISCV_PREFIX=riscv64-unknown-elf-`
2. 将 `doom1.wad` 放到 [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric)。
3. 确认 [`tg-rcore-tutorial-ch8/.cargo/config.toml`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/.cargo/config.toml) 已包含：
   - `-smp 2`
   - `virtio-blk-device`
   - `virtio-gpu-device`
   - `-serial mon:stdio`

### 1. 构建 Doom 用户程序并重新打包镜像
1. 进入 [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric)：
   - 交互版：`make -f Makefile.rcore DG_MODE=interactive`
   - demo 版：`make -f Makefile.rcore DG_MODE=demo`
2. 回到 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8)：
   - `cargo build --features exercise`
3. 如果这一步省略了 `make -f Makefile.rcore`，则 Doom C 侧修复和模式切换都不会进入新的 `fs.img`。

### 2. 单独验证内核态响应中断
1. 在 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8) 执行：
   - `TG_CH8_INIT_APP=kernel_interrupt_check cargo run --features exercise`
2. 通过现象：
   - 出现 `kernel_interrupt_check: hart0 success observed=2`
3. 这说明内核能够在一次长 syscall 中响应至少 2 次 timer interrupt。

### 3. 单独验证多核启动、本地 timer 与共享调度器
1. 在 [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8) 执行：
   - `TG_CH8_INIT_APP=kernel_smp_check cargo run --features exercise`
2. 当前通过现象应包含：
   - `hart0 online (boot), online_harts=1`
   - `hart1 online (secondary), online_harts=2`
   - `online_mask=0x3`
   - `hart0 delta: ticks=...`
   - `hart1 delta: ticks=... kernel_timer_interrupts=...`
   - `user observed hart mask=0x3`
   - `kernel smp check passed!`
3. 其中：
   - `online_mask=0x3` 说明两个 hart 都已上线
   - `user observed hart mask=0x3` 才真正说明多个用户线程已经分别在 `hart0` 和 `hart1` 上运行过
4. 这也是当前最推荐的**用户态多核验证程序**，源码位于 [`tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/kernel_smp_check.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/kernel_smp_check.rs)。

### 3.1 用户态多核性能对照（`smp_bench`）
1. 为了直接比较“同一份工作负载在单核和双核下的耗时差异”，新增了用户态基准程序 [`tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/smp_bench.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/smp_bench.rs)。
2. 其做法是：
   - 先在主线程串行完成 `2` 份 CPU-bound work，得到 `sequential` 时间
   - 再创建 `2` 个用户线程并行完成同样的工作，得到 `parallel` 时间
   - 中途通过 `kernel_hart_snapshot` 记录用户线程实际跑过哪些 hart
3. 构建镜像：
   - `TG_CH8_INIT_APP=smp_bench cargo build --features exercise`
4. 双核运行：
   - `qemu-system-riscv64 -machine virt -serial stdio -monitor none -display none -bios none -smp 2 -m 128M -drive file=target/riscv64gc-unknown-none-elf/debug/fs.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -device virtio-gpu-device,bus=virtio-mmio-bus.1 -kernel target/riscv64gc-unknown-none-elf/debug/jiaxin2006-tg-rcore-tutorial-t1l5`
5. 单核运行：
   - `qemu-system-riscv64 -machine virt -serial stdio -monitor none -display none -bios none -smp 1 -m 128M -drive file=target/riscv64gc-unknown-none-elf/debug/fs.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -device virtio-gpu-device,bus=virtio-mmio-bus.1 -kernel target/riscv64gc-unknown-none-elf/debug/jiaxin2006-tg-rcore-tutorial-t1l5`
6. 关键观察点：
   - 双核下应看到 `online_harts=2 online_mask=0x3`
   - 双核下应看到 `observed_harts=0x3`
   - 单核下应看到 `online_harts=1 online_mask=0x1`
   - 单核下应看到 `observed_harts=0x1`
7. 当前实测结果为：
   - `-smp 1`：`parallel elapsed=5133366 us`，`observed_harts=0x1`，`speedup=0.849x`
   - `-smp 2`：`parallel elapsed=2843263 us`，`observed_harts=0x3`，`speedup=1.732x`
8. 因而以同一份 `parallel` 工作负载直接比较，双核版本相对单核版本约快 `1.80x`；这说明当前“全局 ready queue + per-hart current + 大内核锁”的版本已经能让 CPU-bound 用户线程在两个 hart 上获得实质加速。

### 4. 跑完整回归
1. 执行：
   - `TG_CH8_INIT_APP=ch8_usertest cargo run --features exercise`
2. 若要进一步交给 checker：
   - `TG_CH8_INIT_APP=ch8_usertest cargo run --features exercise > /tmp/ch8_usertest.log 2>&1`
   - `tg-rcore-tutorial-checker --ch 8 --exercise < /tmp/ch8_usertest.log`
3. 通过现象：
   - 日志出现 `ch8 Usertests passed!`
   - checker 返回 `PASS 25/25`

### 5. 运行 Doom 并验证图形 / 输入链路
1. 执行：
   - `cargo run`
2. 图形链路正常时，可在日志中看到：
   - `virtio-gpu: resolution=...`
   - `virtio-gpu: framebuffer initialized`
3. 交互模式下，还会看到：
   - `[doomgeneric] mode=interactive (keyboard from cargo run terminal)`
4. 若切到 demo 模式，则启动参数中会带 `-playdemo demo1`；若切到 interactive 模式，则不再强制自动播放 demo。
5. 需要特别说明的是：Doom 是单线程用户程序，因此**不能单靠 Doom 本身证明“多个用户线程已经在两个 hart 上并行运行”**；这一点仍应以 `kernel_smp_check` 的 `user observed hart mask=0x3` 为准。

### Doom 启动黑屏修复
1. 这轮遇到的“启动后黑屏”并不是 VirtIO-GPU 本身失效，而是 Doom 用户态程序在真正进入图形初始化前就卡住了。
2. 根因主要在 Doom 的 rCore 适配层和简化单机路径：
   - [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/d_loop.c`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/d_loop.c) 里 `D_StartNetGame` 的 `#else` 分支此前漏掉了 `localplayer`、`local_playeringame[]`、`maketic/recvtic/gametic/skiptics` 这些单机循环状态初始化
   - [`tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/d_main.c`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/doomgeneric/doomgeneric/d_main.c) 里启动时还会在 `I_InitGraphics()` 之前先跑一次 `TryRunTics()`，更容易把问题暴露成“卡在首帧前”
   - [`tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/initproc.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/tg-rcore-tutorial-user/src/bin/initproc.rs) 现在也改成了默认直接 `exec("doomgeneric")`，避免 Doom 场景下额外的父进程持续 `wait/yield`
3. 修复后，Doom 的 C 侧程序需要**先单独重编**再重新打包镜像：
   - `cd tg-rcore-tutorial-ch8/doomgeneric/doomgeneric`
   - `make -f Makefile.rcore`
   - `cd ../../`
   - `cargo build` 或 `cargo run`
4. 这一步很关键，因为 `doomgeneric` 是单独的 C 用户程序；只改 Rust 内核代码而不重新 `make`，新的 Doom 侧修复不会进入 `fs.img`。
5. 当前我用无界面 QEMU 连续冷启动 5 次验证，日志都能稳定走到 `I_InitGraphics: ...`，不再卡死在图形初始化之前的黑屏阶段。

### Bug
1. 在 `kernel_trap_handler` 中，为了极小化测试内核中断, 当前只支持 `SupervisorTimer`(参考前面的实现), 但是导致在运行 doom 的时候出现在内核态触发 LoadPageFault 的情况. 
    - 用户态只成功打印了 before init_console，然后就在 init_console 里面炸成了 StorePageFault stval=0x0。这说明根因不在堆分配器，也不在 Doom，本质上是“用户态第一次注册 console/logger 时往空地址写了”。
    - 更准确地说，是当前 `5 ms` timer 仍然足够频繁，比较容易稳定复现这个问题；真正根因不是“时间片太小”，而是用户态在 `init_console` 过程中被 timer 打断后，M 态 trap 返回路径没有恢复原本的 `a0/a1`，导致返回用户态的寄存器/上下文被破坏，后续 `log::set_logger` 里的原子存储才会写到空地址。
    - 进一步定位后发现，真正需要修的是 [`tg-rcore-tutorial-sbi/src/m_entry.asm`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-sbi/src/m_entry.asm) 里的 `m_trap_vector` 返回路径，而不是 Rust 里的 `m_trap_handler` 本体：
      - `m_trap_handler` 负责根据 `mcause` 分发 `ecall` / `machine timer interrupt`，并返回 `SbiRet`
      - 但 `m_trap_vector` 在 `call m_trap_handler` 之后，原先默认把 `a0/a1` 当成返回值保留下来
      - 这只对 **S-mode ecall** 是正确的，因为此时 `a0/a1` 语义就是 SBI 返回值
      - 对 **异步 timer interrupt** 则不对，因为被打断现场里的 `a0/a1` 仍然是当前 S/U 态代码的参数寄存器，返回前必须恢复
      - 因此修复方式是：在 `m_trap_vector` 里先把 `a0/a1` 保存到栈上；`mcause == 9`（S-mode ecall）时保留 `m_trap_handler` 写回的 `a0/a1`，其余中断路径在 `mret` 前恢复原来的 `a0/a1`
2. 调度主循环里原先是直接对 `find_next()` 返回的 `&mut task` 调 `task.context.execute(...)`。这会让主循环层面把线程对象借用跨在整段 `execute -> trap -> 返回调度器` 往返上，可读性很差，也容易让人误判成“外层还持有线程可变借用时又去访问 `PROCESSOR`”。现在已改成：
   - 先用 `find_next_id()` 只选中当前线程 ID
   - 再通过短作用域的 `with_current_task(...)` 闭包读取日志快照和执行 `context.execute(...)`
   - 这样主循环不再直接暴露调度器里线程实体的长生命周期 `&mut` 借用，调度路径更清晰
