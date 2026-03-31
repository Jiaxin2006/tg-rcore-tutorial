# T4 Report

## 内核态响应中断

### 实现方式
1. 在 [`tg-rcore-tutorial-ch8/src/main.rs`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/src/main.rs) 中新增了一套给 **S 态内核代码** 使用的 trap 入口：
   - `kernel_trap_entry`：保存当前内核寄存器现场后跳到 Rust handler
   - `kernel_trap_handler`：当前只处理 `SupervisorTimer`
   - `record_timer_tick` / `NEED_RESCHED` / `KERNEL_TIMER_INTERRUPTS`：记录 tick、置位延迟调度标志、统计内核态 timer interrupt 次数
2. 在 `kernel_trap_handler` 中，当前只支持 `SupervisorTimer`。除了 `SupervisorTimer` 之外的其他 **kernel trap**（包括异常和其他中断）目前都会 panic.
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

### Bug
1. 在 `kernel_trap_handler` 中，为了极小化测试内核中断, 当前只支持 `SupervisorTimer`(参考前面的实现), 但是导致在运行 doom 的时候出现在内核态触发 LoadPageFault 的情况. 
    - 用户态只成功打印了 before init_console，然后就在 init_console 里面炸成了 StorePageFault stval=0x0。这说明根因不在堆分配器，也不在 Doom，本质上是“用户态第一次注册 console/logger 时往空地址写了”。
    - 发现是因为当前时间片设置过小, 用户态在 init_console 过程中被 timer 打断后，返回用户态的寄存器/上下文有破坏，才会把后续 log::set_logger 里的原子存储写到空地址。
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
