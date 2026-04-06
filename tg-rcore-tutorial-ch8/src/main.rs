//! # 第八章：并发
//!
//! 本章在第七章"进程间通信与信号"的基础上，引入了 **线程** 和 **同步原语**。
//!
//! ## 核心概念
//!
//! ### 1. 线程（Thread）
//!
//! 将原来的"进程"拆分为两个独立的抽象：
//! - **Process（进程）**：管理共享资源（地址空间、文件描述符表、同步原语列表、信号）
//! - **Thread（线程）**：管理执行状态（上下文、TID）
//!
//! 同一进程的多个线程共享地址空间，但各自有独立的用户栈和执行上下文。
//!
//! ### 2. 同步原语
//!
//! - **Mutex（互斥锁）**：保证临界区互斥访问
//! - **Semaphore（信号量）**：P/V 操作，支持计数型资源管理
//! - **Condvar（条件变量）**：配合互斥锁使用，支持线程等待/唤醒
//!
//! ### 3. 线程阻塞
//!
//! 当线程尝试获取已被占用的锁/信号量时，内核将其标记为**阻塞态**，
//! 从就绪队列中移除。当持有者释放资源时，唤醒等待队列中的线程。
//!
//! ## 与第七章的主要区别
//!
//! | 特性 | 第七章 | 第八章 |
//! |------|--------|--------|
//! | 执行单元 | Process（进程即线程） | Thread（线程），Process 仅管理资源 |
//! | 管理器 | PManager | PThreadManager（进程 + 线程双层管理） |
//! | 同步 | 无 | Mutex / Semaphore / Condvar |
//! | task-manage feature | `proc` | `thread` |
//! | 新增依赖 | — | tg-sync |
//!
//! 教程阅读建议：
//!
//! - 先看 `rust_main`：抓住“线程创建 + 双层管理 + trap 分发”总流程；
//! - 再看主循环中 `SEMAPHORE_DOWN/MUTEX_LOCK/CONDVAR_WAIT` 分支：理解阻塞态切换；
//! - 最后看 `impls`：把线程、信号、同步三类系统调用如何交织串起来。

// 不使用标准库
#![no_std]
// 不使用默认 main 入口
#![no_main]
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code, unused_imports))]

/// 文件系统模块：easy-fs 封装 + 统一 Fd 枚举
mod fs;
/// 进程与线程模块：Process（资源容器）和 Thread（执行单元）
mod process;
/// 处理器模块：PROCESSOR 全局管理器（PThreadManager）
mod processor;
/// VirtIO 块设备驱动
mod virtio_block;
/// VirtIO-GPU（仅 RISC-V 内核镜像编译）
#[cfg(target_arch = "riscv64")]
mod virtio_gpu;
#[cfg(not(target_arch = "riscv64"))]
mod virtio_gpu {
    pub const DOOM_FB_W: usize = 640;
    pub const DOOM_FB_H: usize = 400;
    pub const DOOM_FB_BYTES: usize = DOOM_FB_W * DOOM_FB_H * 4;
    #[inline]
    pub fn init() -> Result<(), ()> {
        Err(())
    }
    #[inline]
    pub fn dimensions() -> Option<(u32, u32, u32)> {
        None
    }
    #[inline]
    pub fn present(_: &[u8]) -> Result<(), ()> {
        Err(())
    }
}

#[macro_use]
extern crate tg_console;

#[macro_use]
extern crate alloc;

use crate::{
    fs::{FS, read_all},
    impls::{Sv39Manager, SyscallContext},
    process::{Process, Thread},
    processor::{ProcManager, ProcessorInner, ThreadManager},
};
use alloc::alloc::alloc;
use alloc::boxed::Box;
use core::{
    alloc::Layout,
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use impls::Console;
pub use processor::PROCESSOR;
use riscv::register::*;
#[cfg(not(target_arch = "riscv64"))]
use stub::Sv39;
use tg_console::log;
use tg_easy_fs::{FSManager, OpenFlags};
use tg_kernel_context::foreign::MultislotPortal;
#[cfg(target_arch = "riscv64")]
use tg_kernel_vm::page_table::Sv39;
use tg_kernel_vm::{
    AddressSpace,
    page_table::{MmuMeta, PPN, VAddr, VPN, VmFlags, VmMeta},
};
use tg_sbi;
use tg_signal::SignalResult;
use tg_syscall::Caller;
use tg_task_manage::ProcId;
use xmas_elf::ElfFile;

/// 构建 VmFlags
#[cfg(target_arch = "riscv64")]
const fn build_flags(s: &str) -> VmFlags<Sv39> {
    VmFlags::build_from_str(s)
}

/// 解析 VmFlags
#[cfg(target_arch = "riscv64")]
fn parse_flags(s: &str) -> Result<VmFlags<Sv39>, ()> {
    s.parse()
}

#[cfg(not(target_arch = "riscv64"))]
use stub::{build_flags, parse_flags};

const MAX_HARTS: usize = tg_syscall::SMP_HART_CAPACITY;
const BOOT_HART_ID: usize = 0;

// 内核入口，栈 = 32 页 = 128 KiB。
//
// 这里不再调用 tg_linker::boot0! 宏，避免外部已发布版本与 Rust 2024
// 在属性语义上的兼容差异影响本 crate 的发布校验。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 32 * 4096;
    #[unsafe(link_section = ".boot.stack")]
    static mut STACK: [u8; STACK_SIZE * MAX_HARTS] = [0u8; STACK_SIZE * MAX_HARTS];

    core::arch::naked_asm!(
        "mv tp, a0",
        "la sp, {stack}",
        "li t0, {stack_size}",
        "addi t1, a0, 1",
        "mul t1, t1, t0",
        "add sp, sp, t1",
        "j  {main}",
        stack = sym STACK,
        stack_size = const STACK_SIZE,
        main = sym rust_main,
    )
}

/// 内核可用物理内存 = 64 MiB（从 0x80200000 起，不可超过 QEMU -m 总量 - 2M SBI 区域）
const MEMORY: usize = 64 << 20;
/// 异界传送门所在虚页
const PROTAL_TRANSIT: VPN<Sv39> = VPN::MAX;

/// 内核地址空间的全局存储
struct KernelSpace {
    inner: UnsafeCell<MaybeUninit<AddressSpace<Sv39, Sv39Manager>>>,
}

unsafe impl Sync for KernelSpace {}

impl KernelSpace {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    unsafe fn write(&self, space: AddressSpace<Sv39, Sv39Manager>) {
        unsafe { *self.inner.get() = MaybeUninit::new(space) };
    }

    unsafe fn assume_init_ref(&self) -> &AddressSpace<Sv39, Sv39Manager> {
        unsafe { &*(*self.inner.get()).as_ptr() }
    }
}

/// 内核地址空间全局实例
static KERNEL_SPACE: KernelSpace = KernelSpace::new();

/// MMIO 区域：
/// - UART 16550：0x1000_0000
/// - VirtIO-MMIO slots：0x1000_1000..0x1000_9000
pub const MMIO: &[(usize, usize)] = &[(0x1000_0000, 0x1000), (0x1000_1000, 0x8000)];
/// 单次时间片长度（与 ch3 保持一致）。
const TIMER_INTERVAL: u64 = 62_500;
/// 仅调试关键 syscall 路径，避免日志过量影响时序。
static DEBUG_SYSCALL_LOGGED: AtomicBool = AtomicBool::new(false);
/// 限制调度日志数量，避免刷屏影响时序。
static DEBUG_SCHEDULE_LOGS: AtomicU64 = AtomicU64::new(0);
/// 当前已经进入 S 态并完成本核 bring-up 的 hart 数量。
static ONLINE_HARTS: AtomicUsize = AtomicUsize::new(0);
/// online hart 的位图，bit i 表示 hart i 已经完成 bring-up。
static ONLINE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
/// boot hart 完成一次性全局初始化后，放行次核进入 S 态。
#[cfg(target_arch = "riscv64")]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data")]
static SECONDARY_BOOT_READY: AtomicUsize = AtomicUsize::new(0);
/// 第一版 SMP 保护：所有共享内核状态先由一把全局大锁串行化。
static KERNEL_BIG_LOCK: spin::Mutex<()> = spin::Mutex::new(());

/// 每个 hart 的本地运行状态。
struct HartLocalState {
    need_resched: AtomicBool,
    timer_ticks: AtomicU64,
    kernel_timer_interrupts: AtomicU64,
    kernel_timer_logged: AtomicBool,
    kernel_other_interrupt_logged: AtomicBool,
}

impl HartLocalState {
    const fn new() -> Self {
        Self {
            need_resched: AtomicBool::new(false),
            timer_ticks: AtomicU64::new(0),
            kernel_timer_interrupts: AtomicU64::new(0),
            kernel_timer_logged: AtomicBool::new(false),
            kernel_other_interrupt_logged: AtomicBool::new(false),
        }
    }
}

static HART_LOCAL: [HartLocalState; MAX_HARTS] = [
    HartLocalState::new(),
    HartLocalState::new(),
    HartLocalState::new(),
    HartLocalState::new(),
    HartLocalState::new(),
    HartLocalState::new(),
    HartLocalState::new(),
    HartLocalState::new(),
];

#[cfg(target_arch = "riscv64")]
#[inline]
fn current_hart_id() -> usize {
    let hart_id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) hart_id) };
    hart_id
}

#[cfg(not(target_arch = "riscv64"))]
#[inline]
fn current_hart_id() -> usize {
    0
}

#[inline]
fn shared_portal_ptr() -> *mut MultislotPortal {
    PROTAL_TRANSIT.base().val() as *mut MultislotPortal
}

#[inline]
fn hart_local_by_id(hart_id: usize) -> &'static HartLocalState {
    assert!(
        hart_id < MAX_HARTS,
        "hart{hart_id} exceeds MAX_HARTS={MAX_HARTS}"
    );
    &HART_LOCAL[hart_id]
}

#[inline]
fn hart_local() -> &'static HartLocalState {
    hart_local_by_id(current_hart_id())
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn activate_kernel_address_space() {
    let root_ppn = unsafe { KERNEL_SPACE.assume_init_ref() }.root_ppn().val();
    unsafe {
        satp::set(satp::Mode::Sv39, 0, root_ppn);
        core::arch::asm!("sfence.vma");
    }
}

#[cfg(not(target_arch = "riscv64"))]
#[inline]
fn activate_kernel_address_space() {}

#[inline]
fn mark_hart_online(hart_id: usize, role: &str) {
    ONLINE_HART_MASK.fetch_or(1usize << hart_id, Ordering::SeqCst);
    let online = ONLINE_HARTS.fetch_add(1, Ordering::SeqCst) + 1;
    log::info!("hart{hart_id} online ({role}), online_harts={online}");
}

#[inline]
fn fill_hart_snapshot(snapshot: &mut tg_syscall::HartSnapshot) {
    snapshot.max_harts = MAX_HARTS;
    snapshot.online_harts = ONLINE_HARTS.load(Ordering::SeqCst);
    snapshot.online_mask = ONLINE_HART_MASK.load(Ordering::SeqCst);
    snapshot.current_hart = current_hart_id();
    snapshot.need_resched_mask = 0;
    for hart_id in 0..MAX_HARTS {
        let local = hart_local_by_id(hart_id);
        if local.need_resched.load(Ordering::SeqCst) {
            snapshot.need_resched_mask |= 1usize << hart_id;
        }
        snapshot.timer_ticks[hart_id] = local.timer_ticks.load(Ordering::SeqCst);
        snapshot.kernel_timer_interrupts[hart_id] =
            local.kernel_timer_interrupts.load(Ordering::SeqCst);
    }
}

#[inline]
fn program_next_timer() {
    #[cfg(target_arch = "riscv64")]
    tg_sbi::set_timer(time::read64().wrapping_add(TIMER_INTERVAL));
}

#[inline]
fn record_timer_tick() {
    let local = hart_local();
    local.timer_ticks.fetch_add(1, Ordering::SeqCst);
    local.need_resched.store(true, Ordering::SeqCst);
    program_next_timer();
}

#[inline]
fn take_need_resched() -> bool {
    hart_local().need_resched.swap(false, Ordering::SeqCst)
}

#[inline]
fn current_timer_ticks() -> u64 {
    hart_local().timer_ticks.load(Ordering::SeqCst)
}

#[inline]
fn current_kernel_timer_interrupts() -> u64 {
    hart_local().kernel_timer_interrupts.load(Ordering::SeqCst)
}

#[cfg(target_arch = "riscv64")]
#[inline]
fn install_kernel_trap() {
    unsafe {
        stvec::write(
            kernel_trap_entry as *const () as usize,
            stvec::TrapMode::Direct,
        )
    };
}

#[cfg(not(target_arch = "riscv64"))]
#[inline]
fn install_kernel_trap() {}

/// 在较长的内核路径中临时打开中断，让 timer 可以打断 S 态代码。
struct KernelInterruptGuard {
    enabled_before: bool,
    #[cfg(target_arch = "riscv64")]
    stvec_base_before: usize,
    #[cfg(target_arch = "riscv64")]
    stvec_mode_before: stvec::TrapMode,
}

impl KernelInterruptGuard {
    #[inline]
    fn enter() -> Self {
        #[cfg(target_arch = "riscv64")]
        {
            let prev = stvec::read();
            let stvec_base_before = prev.address();
            let stvec_mode_before = prev.trap_mode().unwrap_or(stvec::TrapMode::Direct);
            install_kernel_trap();
            let enabled_before = sstatus::read().sie();
            if !enabled_before {
                unsafe { sstatus::set_sie() };
            }
            Self {
                enabled_before,
                stvec_base_before,
                stvec_mode_before,
            }
        }
        #[cfg(not(target_arch = "riscv64"))]
        {
            Self {
                enabled_before: false,
            }
        }
    }
}

impl Drop for KernelInterruptGuard {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "riscv64")]
        {
            unsafe {
                stvec::write(self.stvec_base_before, self.stvec_mode_before);
            }
            if !self.enabled_before {
                unsafe { sstatus::clear_sie() };
            }
        }
    }
}

#[cfg(target_arch = "riscv64")]
extern "C" fn kernel_trap_handler() {
    use scause::{Interrupt, Trap};
    let hart_id = current_hart_id();
    let local = hart_local();

    match scause::read().cause() {
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            if !local.kernel_timer_logged.swap(true, Ordering::SeqCst) {
                log::debug!("hart{hart_id} kernel timer interrupt observed");
            }
            local.kernel_timer_interrupts.fetch_add(1, Ordering::SeqCst);
            record_timer_tick();
        }
        Trap::Interrupt(cause) => {
            let sepc = sepc::read();
            let stval = stval::read();
            if !local
                .kernel_other_interrupt_logged
                .swap(true, Ordering::SeqCst)
            {
                log::warn!(
                    "unexpected kernel interrupt on hart{hart_id}: {cause:?} sepc={sepc:#x} stval={stval:#x}"
                );
            }
            panic!(
                "unexpected kernel interrupt on hart{hart_id}: {cause:?} sepc={sepc:#x} stval={stval:#x}"
            );
        }
        cause => {
            let sepc = sepc::read();
            let stval = stval::read();
            panic!(
                "unexpected kernel exception on hart{hart_id}: {cause:?} sepc={sepc:#x} stval={stval:#x}"
            );
        }
    }
}

/// S 态内核代码使用的 trap 入口。
///
/// 它只负责保存寄存器并跳到一个很小的 Rust handler，真正的调度仍延后到安全点。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
unsafe extern "C" fn kernel_trap_entry() {
    core::arch::naked_asm!(
        r"  .altmacro
            .macro SAVE n
                sd x\n, \n*8(sp)
            .endm
            .macro SAVE_ALL
                sd x1, 1*8(sp)
                .set n, 3
                .rept 29
                    SAVE %n
                    .set n, n+1
                .endr
            .endm

            .macro LOAD n
                ld x\n, \n*8(sp)
            .endm
            .macro LOAD_ALL
                ld x1, 1*8(sp)
                .set n, 3
                .rept 29
                    LOAD %n
                    .set n, n+1
                .endr
            .endm
        ",
        "   .option push",
        "   .option nopic",
        "   addi sp, sp, -32*8",
        "   SAVE_ALL",
        "   call {handler}",
        "   LOAD_ALL",
        "   addi sp, sp, 32*8",
        "   sret",
        "   .option pop",
        handler = sym kernel_trap_handler,
    )
}

/// 内核主函数
///
/// 与第七章相比：
/// - 新增 `init_thread`（线程系统调用）和 `init_sync_mutex`（同步原语系统调用）
/// - 使用 `PThreadManager`（双层管理器）替代 `PManager`
/// - 初始化时同时创建 Process 和 Thread
/// - 主循环中新增**线程阻塞**处理（SEMAPHORE_DOWN/MUTEX_LOCK/CONDVAR_WAIT）
fn rust_main_secondary(hart_id: usize) -> ! {
    debug_assert_eq!(hart_id, current_hart_id());
    activate_kernel_address_space();
    mark_hart_online(hart_id, "secondary");
    program_next_timer();
    log::debug!("hart{hart_id} local timer armed");
    #[cfg(target_arch = "riscv64")]
    unsafe {
        sie::set_stimer();
    }
    scheduler_loop()
}

#[inline]
fn has_running_threads_elsewhere(processor: *mut ProcessorInner, hart_id: usize) -> bool {
    unsafe { (*processor).has_running_threads_other_than(hart_id) }
}

#[inline]
fn wait_for_work() {
    let _ = take_need_resched();
    #[cfg(target_arch = "riscv64")]
    unsafe {
        install_kernel_trap();
        sie::set_stimer();
        sstatus::set_sie();
        core::arch::asm!("wfi");
        sstatus::clear_sie();
    }
    #[cfg(not(target_arch = "riscv64"))]
    core::hint::spin_loop();
}

#[inline]
unsafe fn execute_current_on_hart(
    processor: *mut ProcessorInner,
    hart_id: usize,
) {
    let task = unsafe {
        (*processor)
            .with_current_task_for(hart_id, |task| &mut **task as *mut Thread)
            .unwrap()
    };
    // SAFETY:
    // - 调度器保证同一时刻一个线程只会出现在一个 hart 的 current 槽里；
    // - 共享 portal 预留了按 hart 划分的 slot，执行时使用 `hart_id` 作为 slot key，
    //   因此不同 hart 只会写各自的 PortalCache；
    // - 这里在释放大内核锁后进入用户态，使多个 hart 可以并行执行用户线程，
    //   但重新进入内核后仍会先争用同一把锁，先保证共享内核状态正确。
    unsafe {
        (*task).context.execute(&mut *shared_portal_ptr(), hart_id);
    }
}

fn scheduler_loop() -> ! {
    loop {
        let hart_id = current_hart_id();
        let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
        let mut kernel_lock = KERNEL_BIG_LOCK.lock();
        if let Some(_tid) = unsafe { (*processor).find_next_id_for(hart_id) } {
            let schedule_log_index = DEBUG_SCHEDULE_LOGS.fetch_add(1, Ordering::SeqCst);
            if schedule_log_index < 24 {
                let (tid, pc, sp, ra) = unsafe {
                    (*processor)
                        .with_current_task_for(hart_id, |task| {
                            (
                                task.tid,
                                task.context.context.pc(),
                                task.context.context.sp(),
                                task.context.context.ra(),
                            )
                        })
                        .unwrap()
                };
                log::debug!(
                    "schedule: hart{hart_id} tid={tid:?} pc={pc:#x} sp={sp:#x} ra={ra:#x}"
                );
            }
            drop(kernel_lock);
            unsafe { execute_current_on_hart(processor, hart_id) };
            kernel_lock = KERNEL_BIG_LOCK.lock();
            let cause = scause::read().cause();
            match cause {
                scause::Trap::Interrupt(scause::Interrupt::SupervisorTimer) => {
                    record_timer_tick();
                    if take_need_resched() {
                        unsafe { (*processor).make_current_suspend_for(hart_id) };
                    }
                }
                scause::Trap::Exception(scause::Exception::UserEnvCall) => {
                    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};
                    let (id, args, sepc_before) = unsafe {
                        (*processor)
                            .with_current_task_for(hart_id, |task| {
                                let ctx = &mut task.context.context;
                                let sepc_before = ctx.pc();
                                ctx.move_next();
                                (
                                    ctx.a(7).into(),
                                    [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)],
                                    sepc_before,
                                )
                            })
                            .unwrap()
                    };
                    if matches!(
                        id,
                        Id::CLOCK_GETTIME
                            | Id::KERNEL_INTERRUPT_CHECK
                            | Id::KERNEL_HART_SNAPSHOT
                            | Id::EXIT
                            | Id::CLONE
                            | Id::WAIT4
                            | Id::EXECVE
                    ) {
                        log::debug!(
                            "syscall enter: hart{hart_id} id={id:?} sepc={sepc_before:#x} a0={:#x} a1={:#x}",
                            args[0],
                            args[1],
                        );
                    } else if !DEBUG_SYSCALL_LOGGED.swap(true, Ordering::SeqCst) {
                        log::debug!(
                            "first non-debug syscall seen: hart{hart_id} id={id:?} sepc={sepc_before:#x}"
                        );
                    }
                    let syscall_ret = tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args);
                    if matches!(
                        id,
                        Id::CLOCK_GETTIME
                            | Id::KERNEL_INTERRUPT_CHECK
                            | Id::KERNEL_HART_SNAPSHOT
                            | Id::EXIT
                            | Id::CLONE
                            | Id::WAIT4
                            | Id::EXECVE
                    ) {
                        match syscall_ret {
                            Ret::Done(ret) => {
                                log::debug!("syscall return: hart{hart_id} id={id:?} ret={ret}")
                            }
                            Ret::Unsupported(unsupported) => {
                                log::debug!(
                                    "syscall return: hart{hart_id} id={id:?} unsupported={unsupported:?}"
                                );
                            }
                        }
                    }

                    let signal_result = unsafe {
                        (*processor)
                            .with_current_task_proc_for(hart_id, |task, current_proc| {
                                current_proc
                                    .signal
                                    .handle_signals(&mut task.context.context)
                            })
                            .unwrap()
                    };
                    match signal_result {
                        SignalResult::ProcessKilled(exit_code) => unsafe {
                            (*processor).make_current_exited_for(hart_id, exit_code as _)
                        },
                        _ => match syscall_ret {
                            Ret::Done(ret) => match id {
                                Id::EXIT => unsafe { (*processor).make_current_exited_for(hart_id, ret) },
                                Id::SEMAPHORE_DOWN
                                | Id::CONDVAR_WAIT
                                | Id::RWLOCK_READ_LOCK
                                | Id::RWLOCK_WRITE_LOCK => {
                                    unsafe {
                                        (*processor)
                                            .with_current_task_for(hart_id, |task| {
                                                *task.context.context.a_mut(0) = ret as _;
                                            })
                                            .unwrap();
                                    }
                                    if ret == -1 {
                                        unsafe { (*processor).make_current_blocked_for(hart_id) };
                                    } else {
                                        unsafe { (*processor).make_current_suspend_for(hart_id) };
                                    }
                                }
                                Id::MUTEX_LOCK => {
                                    unsafe {
                                        (*processor)
                                            .with_current_task_for(hart_id, |task| {
                                                *task.context.context.a_mut(0) = ret as _;
                                            })
                                            .unwrap();
                                    }
                                    if ret == -1 {
                                        unsafe { (*processor).make_current_blocked_for(hart_id) };
                                    } else {
                                        unsafe { (*processor).make_current_suspend_for(hart_id) };
                                    }
                                }
                                _ => {
                                    unsafe {
                                        (*processor)
                                            .with_current_task_for(hart_id, |task| {
                                                *task.context.context.a_mut(0) = ret as _;
                                            })
                                            .unwrap();
                                    }
                                    unsafe { (*processor).make_current_suspend_for(hart_id) };
                                }
                            },
                            Ret::Unsupported(_) => {
                                log::info!("id = {id:?}");
                                unsafe { (*processor).make_current_exited_for(hart_id, -2) };
                            }
                        },
                    }
                }
                e => {
                    let sepc = riscv::register::sepc::read();
                    let stval = riscv::register::stval::read();
                    log::error!("unsupported trap on hart{hart_id}: {e:?} sepc={sepc:#x} stval={stval:#x}");
                    unsafe { (*processor).make_current_exited_for(hart_id, -3) };
                }
            }
        } else {
            let no_running_threads = !unsafe { (*processor).has_running_threads() };
            drop(kernel_lock);
            if no_running_threads {
                if hart_id == BOOT_HART_ID {
                    println!("no task");
                    tg_sbi::shutdown(false);
                }
                loop {
                    wait_for_work();
                }
            }
            if !has_running_threads_elsewhere(processor, hart_id) {
                core::hint::spin_loop();
            }
            wait_for_work();
        }
    }
}

extern "C" fn rust_main(hart_id: usize) -> ! {
    debug_assert_eq!(hart_id, current_hart_id());
    if hart_id != BOOT_HART_ID {
        rust_main_secondary(hart_id);
    }

    let layout = tg_linker::KernelLayout::locate();
    // 步骤 1：BSS 清零
    unsafe { layout.zero_bss() };
    // 步骤 2：控制台和日志
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG").or(Some("info")));
    mark_hart_online(hart_id, "boot");
    // 步骤 3：堆分配器
    tg_kernel_alloc::init(layout.start() as _);
    unsafe {
        tg_kernel_alloc::transfer(core::slice::from_raw_parts_mut(
            layout.end() as _,
            MEMORY - layout.len(),
        ))
    };
    // 步骤 4：异界传送门
    let portal_size = MultislotPortal::calculate_size(MAX_HARTS);
    let portal_layout = Layout::from_size_align(portal_size, 1 << Sv39::PAGE_BITS).unwrap();
    let portal_ptr = unsafe { alloc(portal_layout) };
    assert!(portal_layout.size() < 1 << Sv39::PAGE_BITS);
    // 步骤 5：内核地址空间
    kernel_space(layout, MEMORY, portal_ptr as _);
    // 步骤 6：异界传送门初始化
    let _portal = unsafe { MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), MAX_HARTS) };
    // 步骤 7：系统调用初始化
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_signal(&SyscallContext);
    tg_syscall::init_thread(&SyscallContext); // 本章新增：线程系统调用
    tg_syscall::init_sync_mutex(&SyscallContext); // 本章新增：同步原语系统调用
    tg_syscall::init_kernel_test(&SyscallContext); // T4 新增：内核态中断检查接口
    // 步骤 7b：VirtIO-GPU 帧缓冲（可选，供 doomgeneric / fb_demo）
    match crate::virtio_gpu::init() {
        Ok(()) => log::info!("virtio-gpu: framebuffer initialized"),
        Err(()) => log::warn!("virtio-gpu: unavailable (no GPU in this machine?)"),
    }
    tg_syscall::init_framebuffer(&SyscallContext);
    // 步骤 8：加载初始用户程序（默认 initproc，调试时可用 TG_CH8_INIT_APP 覆盖）
    let init_app = option_env!("TG_CH8_INIT_APP").unwrap_or("initproc");
    let init_app_data = read_all(FS.open(init_app, OpenFlags::RDONLY).unwrap());
    if let Some((process, thread)) =
        Process::from_elf(ElfFile::new(init_app_data.as_slice()).unwrap())
    {
        // 初始化双层管理器：ProcManager（进程）+ ThreadManager（线程）
        PROCESSOR.get_mut().set_proc_manager(ProcManager::new());
        PROCESSOR.get_mut().set_manager(ThreadManager::new());
        let (pid, tid) = (process.pid, thread.tid);
        PROCESSOR
            .get_mut()
            .add_proc(pid, process, ProcId::from_usize(usize::MAX));
        PROCESSOR
            .get_mut()
            .add_for(BOOT_HART_ID, tid, Box::new(thread), pid);
    }
    unsafe { sie::set_stimer() };
    program_next_timer();
    #[cfg(target_arch = "riscv64")]
    {
        SECONDARY_BOOT_READY.store(1, Ordering::SeqCst);
        unsafe { core::arch::asm!("fence rw, rw") };
        log::info!("boot hart released secondary harts");
    }
    scheduler_loop()
}

/// panic 处理
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

/// 建立内核地址空间（与前几章相同）
fn kernel_space(layout: tg_linker::KernelLayout, memory: usize, portal: usize) {
    let mut space = AddressSpace::new();
    for region in layout.iter() {
        log::info!("{region}");
        use tg_linker::KernelRegionTitle::*;
        let flags = match region.title {
            Text => "X_RV",
            Rodata => "__RV",
            Data | Boot => "_WRV",
        };
        let s = VAddr::<Sv39>::new(region.range.start);
        let e = VAddr::<Sv39>::new(region.range.end);
        space.map_extern(
            s.floor()..e.ceil(),
            PPN::new(s.floor().val()),
            build_flags(flags),
        )
    }
    let s = VAddr::<Sv39>::new(layout.end());
    let e = VAddr::<Sv39>::new(layout.start() + memory);
    log::info!("(heap) ---> {:#10x}..{:#10x}", s.val(), e.val());
    space.map_extern(
        s.floor()..e.ceil(),
        PPN::new(s.floor().val()),
        build_flags("_WRV"),
    );
    space.map_extern(
        PROTAL_TRANSIT..PROTAL_TRANSIT + 1,
        PPN::new(portal >> Sv39::PAGE_BITS),
        build_flags("__G_XWRV"),
    );
    println!();
    for (base, len) in MMIO {
        let s = VAddr::<Sv39>::new(*base);
        let e = VAddr::<Sv39>::new(*base + *len);
        log::info!("MMIO range -> {:#10x}..{:#10x}", s.val(), e.val());
        space.map_extern(
            s.floor()..e.ceil(),
            PPN::new(s.floor().val()),
            build_flags("_WRV"),
        );
    }
    unsafe { satp::set(satp::Mode::Sv39, 0, space.root_ppn().val()) };
    unsafe { KERNEL_SPACE.write(space) };
}

/// 将异界传送门映射到用户地址空间
fn map_portal(space: &AddressSpace<Sv39, Sv39Manager>) {
    let portal_idx = PROTAL_TRANSIT.index_in(Sv39::MAX_LEVEL);
    space.root()[portal_idx] = unsafe { KERNEL_SPACE.assume_init_ref() }.root()[portal_idx];
}

/// 各种接口库的实现
///
/// 与第七章相比，本章新增了：
/// - `Thread` trait（thread_create/gettid/waittid）
/// - `SyncMutex` trait（mutex/semaphore/condvar 系统调用）
/// - 所有操作通过 `ProcessorInner`（PThreadManager）进行双层管理
mod impls {
    use crate::{
        KernelInterruptGuard, PROCESSOR, Sv39, TIMER_INTERVAL, Thread, build_flags,
        current_hart_id, current_kernel_timer_interrupts, current_timer_ticks, fill_hart_snapshot,
        fs::{FS, Fd, read_all},
        processor::ProcessorInner,
        program_next_timer,
    };
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::sync::Arc;
    use alloc::{alloc::alloc_zeroed, string::String, vec::Vec};
    use core::{alloc::Layout, cmp::min, ptr::NonNull};
    use spin::Mutex;
    use tg_console::log;
    use tg_easy_fs::{FSManager, OpenFlags, UserBuffer, make_pipe};
    use tg_kernel_vm::{
        AddressSpace, PageManager,
        page_table::{MmuMeta, PPN, Pte, VAddr, VPN, VmFlags, VmMeta},
    };
    use tg_signal::SignalNo;
    use tg_sync::{Condvar, Mutex as MutexTrait, MutexBlocking, RwLock, Semaphore, SpinLock};
    use tg_syscall::*;
    use tg_task_manage::{ProcId, ThreadId};
    use xmas_elf::ElfFile;

    // ─── Sv39 页表管理器 ───

    /// Sv39 页表管理器
    #[repr(transparent)]
    pub struct Sv39Manager(NonNull<Pte<Sv39>>);

    impl Sv39Manager {
        const OWNED: VmFlags<Sv39> = unsafe { VmFlags::from_raw(1 << 8) };
        #[inline]
        fn page_alloc<T>(count: usize) -> *mut T {
            unsafe {
                alloc_zeroed(Layout::from_size_align_unchecked(
                    count << Sv39::PAGE_BITS,
                    1 << Sv39::PAGE_BITS,
                ))
            }
            .cast()
        }
    }

    impl PageManager<Sv39> for Sv39Manager {
        #[inline]
        fn new_root() -> Self {
            Self(NonNull::new(Self::page_alloc(1)).unwrap())
        }
        #[inline]
        fn root_ppn(&self) -> PPN<Sv39> {
            PPN::new(self.0.as_ptr() as usize >> Sv39::PAGE_BITS)
        }
        #[inline]
        fn root_ptr(&self) -> NonNull<Pte<Sv39>> {
            self.0
        }
        #[inline]
        fn p_to_v<T>(&self, ppn: PPN<Sv39>) -> NonNull<T> {
            unsafe { NonNull::new_unchecked(VPN::<Sv39>::new(ppn.val()).base().as_mut_ptr()) }
        }
        #[inline]
        fn v_to_p<T>(&self, ptr: NonNull<T>) -> PPN<Sv39> {
            PPN::new(VAddr::<Sv39>::new(ptr.as_ptr() as _).floor().val())
        }
        #[inline]
        fn check_owned(&self, pte: Pte<Sv39>) -> bool {
            pte.flags().contains(Self::OWNED)
        }
        #[inline]
        fn allocate(&mut self, len: usize, flags: &mut VmFlags<Sv39>) -> NonNull<u8> {
            *flags |= Self::OWNED;
            NonNull::new(Self::page_alloc(len)).unwrap()
        }
        fn deallocate(&mut self, _pte: Pte<Sv39>, _len: usize) -> usize {
            todo!()
        }
        fn drop_root(&mut self) {
            todo!()
        }
    }

    // ─── 控制台 ───

    /// 控制台实现
    pub struct Console;
    impl tg_console::Console for Console {
        #[inline]
        fn put_char(&self, c: u8) {
            tg_sbi::console_putchar(c);
        }
    }

    // ─── 系统调用实现 ───

    /// 系统调用上下文
    pub struct SyscallContext;
    const READABLE: VmFlags<Sv39> = build_flags("RV");
    const WRITEABLE: VmFlags<Sv39> = build_flags("W_V");
    const DEADLOCK_RET: isize = -0xDEAD;
    static FB_UPLOAD_BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    fn copy_from_user(
        address_space: &AddressSpace<Sv39, Sv39Manager>,
        user_buf: usize,
        dst: &mut [u8],
    ) -> bool {
        let page_size = 1usize << Sv39::PAGE_BITS;
        let mut copied = 0usize;
        while copied < dst.len() {
            let Some(user_addr) = user_buf.checked_add(copied) else {
                return false;
            };
            let offset = user_addr & (page_size - 1);
            let chunk = min(page_size - offset, dst.len() - copied);
            let Some(src) = address_space.translate::<u8>(VAddr::new(user_addr), READABLE) else {
                return false;
            };
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr().add(copied), chunk);
            }
            copied += chunk;
        }
        true
    }

    fn copy_to_user(
        address_space: &AddressSpace<Sv39, Sv39Manager>,
        user_buf: usize,
        src: &[u8],
    ) -> bool {
        let page_size = 1usize << Sv39::PAGE_BITS;
        let mut copied = 0usize;
        while copied < src.len() {
            let Some(user_addr) = user_buf.checked_add(copied) else {
                return false;
            };
            let offset = user_addr & (page_size - 1);
            let chunk = min(page_size - offset, src.len() - copied);
            let Some(dst) = address_space.translate::<u8>(VAddr::new(user_addr), WRITEABLE) else {
                return false;
            };
            unsafe {
                core::ptr::copy_nonoverlapping(src.as_ptr().add(copied), dst.as_ptr(), chunk);
            }
            copied += chunk;
        }
        true
    }

    fn is_unsafe_state(
        available: &[usize],
        allocation: &[Vec<usize>],
        need: &[Vec<usize>],
    ) -> bool {
        let n = allocation.len();
        let m = available.len();
        let mut work = available.to_vec();
        let mut finish = vec![false; n];
        loop {
            let mut progress = false;
            for i in 0..n {
                if finish[i] {
                    continue;
                }
                let can_finish = (0..m).all(|j| need[i][j] <= work[j]);
                if can_finish {
                    for (j, w) in work.iter_mut().enumerate().take(m) {
                        *w += allocation[i][j];
                    }
                    finish[i] = true;
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        finish.iter().any(|ok| !ok)
    }

    fn detect_mutex_deadlock(
        processor: *mut ProcessorInner,
        pid: ProcId,
        request_tid: ThreadId,
        request_mutex_id: usize,
    ) -> bool {
        let tids = unsafe {
            (*processor)
                .get_thread(pid)
                .map(|v| v.clone())
                .unwrap_or_default()
        };
        if tids.is_empty() {
            return false;
        }
        let mut tid_to_idx = BTreeMap::new();
        for (idx, tid) in tids.iter().enumerate() {
            tid_to_idx.insert(*tid, idx);
        }

        let mutex_ids = {
            let proc = unsafe { (*processor).get_proc(pid).unwrap() };
            proc.mutex_list
                .iter()
                .enumerate()
                .filter_map(|(id, item)| item.as_ref().map(|_| id))
                .collect::<Vec<usize>>()
        };
        let m = mutex_ids.len();
        if m == 0 {
            return false;
        }
        let req_col = match mutex_ids.iter().position(|id| *id == request_mutex_id) {
            Some(v) => v,
            None => return false,
        };
        let n = tids.len();
        let mut available = vec![0usize; m];
        let mut allocation = vec![vec![0usize; m]; n];
        let mut need = vec![vec![0usize; m]; n];

        {
            let proc = unsafe { (*processor).get_proc(pid).unwrap() };
            for (col, mutex_id) in mutex_ids.iter().enumerate() {
                let mutex = match proc.mutex_list[*mutex_id].as_ref() {
                    Some(mutex) => Arc::clone(mutex),
                    None => continue,
                };
                if let Some(holder) = mutex.holder() {
                    if let Some(&row) = tid_to_idx.get(&holder) {
                        allocation[row][col] = 1;
                    }
                } else {
                    available[col] = 1;
                }
                for waiter in mutex.waiting() {
                    if let Some(&row) = tid_to_idx.get(&waiter) {
                        need[row][col] = 1;
                    }
                }
            }
        }

        if let Some(&row) = tid_to_idx.get(&request_tid) {
            need[row][req_col] += 1;
        }

        is_unsafe_state(&available, &allocation, &need)
    }

    fn detect_semaphore_deadlock(
        processor: *mut ProcessorInner,
        pid: ProcId,
        request_tid: ThreadId,
        request_sem_id: usize,
    ) -> bool {
        let tids = unsafe {
            (*processor)
                .get_thread(pid)
                .map(|v| v.clone())
                .unwrap_or_default()
        };
        if tids.is_empty() {
            return false;
        }
        let mut tid_to_idx = BTreeMap::new();
        for (idx, tid) in tids.iter().enumerate() {
            tid_to_idx.insert(*tid, idx);
        }

        let sem_ids = {
            let proc = unsafe { (*processor).get_proc(pid).unwrap() };
            proc.semaphore_list
                .iter()
                .enumerate()
                .filter_map(|(id, item)| item.as_ref().map(|_| id))
                .collect::<Vec<usize>>()
        };
        let m = sem_ids.len();
        if m == 0 {
            return false;
        }
        let req_col = match sem_ids.iter().position(|id| *id == request_sem_id) {
            Some(v) => v,
            None => return false,
        };
        let n = tids.len();
        let mut available = vec![0usize; m];
        let mut allocation = vec![vec![0usize; m]; n];
        let mut need = vec![vec![0usize; m]; n];

        {
            let proc = unsafe { (*processor).get_proc(pid).unwrap() };
            for (col, sem_id) in sem_ids.iter().enumerate() {
                let sem = match proc.semaphore_list[*sem_id].as_ref() {
                    Some(sem) => Arc::clone(sem),
                    None => continue,
                };
                let (avail, wait_queue, alloc_map) = sem.deadlock_snapshot();
                available[col] = avail;
                for (tid, cnt) in &alloc_map {
                    if let Some(&row) = tid_to_idx.get(tid) {
                        allocation[row][col] = *cnt;
                    }
                }
                for waiter in wait_queue {
                    if let Some(&row) = tid_to_idx.get(&waiter) {
                        need[row][col] += 1;
                    }
                }
            }
        }

        if let Some(&row) = tid_to_idx.get(&request_tid) {
            need[row][req_col] += 1;
        }

        is_unsafe_state(&available, &allocation, &need)
    }

    /// IO 系统调用（与第七章基本相同，本章新增 lseek / fstat 支持 Doom 文件 I/O）
    ///
    /// 注意：本章通过 `get_current_proc_for(current_hart_id())` 获取当前线程所属的进程，
    /// 而非直接 `current()`，因为 fd_table 属于进程而非线程。
    impl IO for SyscallContext {
        fn input_getchar(&self, _caller: Caller) -> isize {
            const UART_BASE: usize = 0x1000_0000;
            const UART_RBR: usize = UART_BASE;
            const UART_LSR: usize = UART_BASE + 5;
            let lsr = unsafe { (UART_LSR as *const u8).read_volatile() };
            if (lsr & 1) == 0 {
                -1
            } else {
                unsafe { (UART_RBR as *const u8).read_volatile() as isize }
            }
        }

        fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if let Some(ptr) = current.address_space.translate(VAddr::new(buf), READABLE) {
                if fd == STDOUT || fd == STDDEBUG {
                    print!("{}", unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            ptr.as_ptr(),
                            count,
                        ))
                    });
                    count as _
                } else if let Some(file) = &current.fd_table[fd] {
                    let file = file.lock();
                    if file.writable() {
                        let mut v: Vec<&'static mut [u8]> = Vec::new();
                        unsafe { v.push(core::slice::from_raw_parts_mut(ptr.as_ptr(), count)) };
                        file.write(UserBuffer::new(v)) as _
                    } else {
                        log::error!("file not writable");
                        -1
                    }
                } else {
                    log::error!("unsupported fd: {fd}");
                    -1
                }
            } else {
                log::error!("ptr not readable");
                -1
            }
        }

        fn read(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if let Some(ptr) = current.address_space.translate(VAddr::new(buf), WRITEABLE) {
                if fd == STDIN {
                    let mut ptr = ptr.as_ptr();
                    for _ in 0..count {
                        unsafe {
                            *ptr = tg_sbi::console_getchar() as u8;
                            ptr = ptr.add(1);
                        }
                    }
                    count as _
                } else if let Some(file) = &current.fd_table[fd] {
                    let file = file.lock();
                    if file.readable() {
                        let mut v: Vec<&'static mut [u8]> = Vec::new();
                        unsafe { v.push(core::slice::from_raw_parts_mut(ptr.as_ptr(), count)) };
                        file.read(UserBuffer::new(v)) as _
                    } else {
                        log::error!("file not readable");
                        -1
                    }
                } else {
                    log::error!("unsupported fd: {fd}");
                    -1
                }
            } else {
                log::error!("ptr not writeable");
                -1
            }
        }

        fn open(&self, _caller: Caller, path: usize, flags: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if let Some(ptr) = current.address_space.translate(VAddr::new(path), READABLE) {
                let mut string = String::new();
                let mut raw_ptr: *mut u8 = ptr.as_ptr();
                loop {
                    unsafe {
                        let ch = *raw_ptr;
                        if ch == 0 {
                            break;
                        }
                        string.push(ch as char);
                        raw_ptr = (raw_ptr as usize + 1) as *mut u8;
                    }
                }
                if let Some(file_handle) =
                    FS.open(string.as_str(), OpenFlags::from_bits(flags as u32).unwrap())
                {
                    let new_fd = current.fd_table.len();
                    current
                        .fd_table
                        .push(Some(Mutex::new(Fd::File((*file_handle).clone()))));
                    new_fd as isize
                } else {
                    -1
                }
            } else {
                log::error!("ptr not writeable");
                -1
            }
        }

        #[inline]
        fn close(&self, _caller: Caller, fd: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if fd >= current.fd_table.len() || current.fd_table[fd].is_none() {
                return -1;
            }
            current.fd_table[fd].take();
            0
        }

        /// pipe 系统调用
        fn pipe(&self, _caller: Caller, pipe: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            let (read_end, write_end) = make_pipe();
            let read_fd = current.fd_table.len();
            let write_fd = read_fd + 1;
            if let Some(mut ptr) = current
                .address_space
                .translate::<usize>(VAddr::new(pipe), WRITEABLE)
            {
                unsafe { *ptr.as_mut() = read_fd };
            } else {
                return -1;
            }
            if let Some(mut ptr) = current
                .address_space
                .translate::<usize>(VAddr::new(pipe + core::mem::size_of::<usize>()), WRITEABLE)
            {
                unsafe { *ptr.as_mut() = write_fd };
            } else {
                return -1;
            }
            current
                .fd_table
                .push(Some(Mutex::new(Fd::PipeRead(read_end))));
            current
                .fd_table
                .push(Some(Mutex::new(Fd::PipeWrite(write_end))));
            0
        }

        fn lseek(&self, _caller: Caller, fd: usize, offset: isize, whence: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if fd >= current.fd_table.len() || current.fd_table[fd].is_none() {
                return -1;
            }
            current.fd_table[fd]
                .as_ref()
                .unwrap()
                .lock()
                .lseek(offset, whence)
        }

        fn fstat(&self, _caller: Caller, fd: usize, st: usize) -> isize {
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if fd >= current.fd_table.len() || current.fd_table[fd].is_none() {
                return -1;
            }
            let file_size = current.fd_table[fd].as_ref().unwrap().lock().file_size();
            let is_file = file_size > 0;
            if let Some(mut ptr) = current
                .address_space
                .translate::<Stat>(VAddr::new(st), WRITEABLE)
            {
                let stat = unsafe { ptr.as_mut() };
                *stat = Stat::new();
                if is_file {
                    stat.mode = StatMode::FILE;
                }
                0
            } else {
                -1
            }
        }
    }

    /// 进程管理系统调用
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, exit_code: usize) -> isize {
            exit_code as isize
        }

        fn sbrk(&self, _caller: Caller, _size: i32) -> isize {
            -1
        }

        /// fork：创建子进程（返回 Process + Thread）
        fn fork(&self, _caller: Caller) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let (parent_pid, proc, mut thread) = unsafe {
                (*processor)
                    .with_current_proc_for(current_hart_id(), |current_proc| {
                        let parent_pid = current_proc.pid;
                        let (proc, thread) = current_proc.fork().unwrap();
                        (parent_pid, proc, thread)
                    })
                    .unwrap()
            };
            let pid = proc.pid;
            *thread.context.context.a_mut(0) = 0 as _;
            unsafe {
                (*processor).add_proc(pid, proc, parent_pid);
                (*processor).add_for(current_hart_id(), thread.tid, Box::new(thread), pid);
            }
            pid.get_usize() as isize
        }

        /// exec：从文件系统加载新程序
        fn exec(&self, _caller: Caller, path: usize, count: usize) -> isize {
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            current
                .address_space
                .translate(VAddr::new(path), READABLE)
                .map(|ptr| unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr.as_ptr(), count))
                })
                .and_then(|name| {
                    log::debug!(
                        "exec request: hart{} pid={:?} path='{}' len={}",
                        current_hart_id(),
                        current.pid,
                        name,
                        count
                    );
                    FS.open(name, OpenFlags::RDONLY).map(|fd| (name, fd))
                })
                .map_or_else(
                    || {
                        log::error!("unknown app, select one in the list: ");
                        FS.readdir("")
                            .unwrap()
                            .into_iter()
                            .for_each(|app| println!("{app}"));
                        println!();
                        -1
                    },
                    |(name, fd)| {
                        let data = read_all(fd);
                        log::debug!("exec: path='{}' read {} bytes", name, data.len());
                        let elf = ElfFile::new(&data).unwrap();
                        log::debug!(
                            "exec: ELF parsed, entry={:#x}",
                            elf.header.pt2.entry_point()
                        );
                        current.exec(elf);
                        log::debug!("exec: address space replaced, resuming user");
                        0
                    },
                )
        }

        fn wait(&self, _caller: Caller, pid: isize, exit_code_ptr: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
            if let Some((dead_pid, exit_code)) =
                unsafe { (*processor).wait_for(current_hart_id(), ProcId::from_usize(pid as usize)) }
            {
                let wrote = unsafe {
                    (*processor)
                        .with_current_proc_for(current_hart_id(), |current| {
                            current
                                .address_space
                                .translate::<i32>(VAddr::new(exit_code_ptr), WRITABLE)
                                .map(|mut ptr| {
                                    *ptr.as_mut() = exit_code as i32;
                                })
                                .is_some()
                        })
                        .unwrap()
                };
                if !wrote {
                    return -1;
                }
                dead_pid.get_usize() as isize
            } else {
                return -1;
            }
        }

        fn getpid(&self, _caller: Caller) -> isize {
            PROCESSOR
                .get_mut()
                .current_pid_for(current_hart_id())
                .unwrap()
                .get_usize() as _
        }
    }

    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }
    }

    impl Clock for SyscallContext {
        #[inline]
        fn clock_gettime(&self, _caller: Caller, clock_id: ClockId, tp: usize) -> isize {
            const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
            match clock_id {
                ClockId::CLOCK_MONOTONIC => {
                    log::debug!("clock_gettime: tp={tp:#x}");
                    if let Some(mut ptr) = PROCESSOR
                        .get_mut()
                        .get_current_proc_for(current_hart_id())
                        .unwrap()
                        .address_space
                        .translate(VAddr::new(tp), WRITABLE)
                    {
                        let time = riscv::register::time::read() * 10000 / 125;
                        *unsafe { ptr.as_mut() } = TimeSpec {
                            tv_sec: time / 1_000_000_000,
                            tv_nsec: time % 1_000_000_000,
                        };
                        0
                    } else {
                        log::error!("ptr not readable");
                        -1
                    }
                }
                _ => -1,
            }
        }
    }

    impl tg_syscall::KernelTest for SyscallContext {
        fn kernel_interrupt_check(&self, _caller: Caller, min_interrupts: usize) -> isize {
            let hart_id = current_hart_id();
            let min_interrupts = min_interrupts.max(1) as u64;
            let start_interrupts = current_kernel_timer_interrupts();
            let start_time = riscv::register::time::read64();
            let timeout = TIMER_INTERVAL.saturating_mul(min_interrupts + 32);
            log::debug!(
                "kernel_interrupt_check: hart{hart_id} enter min_interrupts={} start_interrupts={} start_time={}",
                min_interrupts,
                start_interrupts,
                start_time,
            );
            let _interrupt_guard = KernelInterruptGuard::enter();
            unsafe {
                riscv::register::sie::set_stimer();
            }
            program_next_timer();

            loop {
                let observed = current_kernel_timer_interrupts().saturating_sub(start_interrupts);
                if observed >= min_interrupts {
                    log::info!("kernel_interrupt_check: hart{hart_id} success observed={observed}");
                    return observed as isize;
                }
                if riscv::register::time::read64().wrapping_sub(start_time) >= timeout {
                    let sie = riscv::register::sie::read();
                    let sip = riscv::register::sip::read();
                    let sstatus = riscv::register::sstatus::read();
                    log::warn!(
                        "kernel_interrupt_check timeout on hart{hart_id}: observed={observed} sie.stimer={} sip.stimer={} sstatus.sie={} ticks={}",
                        sie.stimer(),
                        sip.stimer(),
                        sstatus.sie(),
                        current_timer_ticks(),
                    );
                    return observed as isize;
                }
                for _ in 0..4096 {
                    core::hint::spin_loop();
                }
            }
        }

        fn kernel_hart_snapshot(&self, _caller: Caller, out_ptr: usize, out_len: usize) -> isize {
            if out_len < core::mem::size_of::<HartSnapshot>() {
                return -1;
            }
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            let mut snapshot = HartSnapshot::default();
            fill_hart_snapshot(&mut snapshot);
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&snapshot as *const HartSnapshot).cast::<u8>(),
                    core::mem::size_of::<HartSnapshot>(),
                )
            };
            if copy_to_user(&current.address_space, out_ptr, bytes) {
                snapshot.online_harts as isize
            } else {
                -1
            }
        }
    }

    /// 信号系统调用（与第七章相同）
    impl Signal for SyscallContext {
        fn kill(&self, _caller: Caller, pid: isize, signum: u8) -> isize {
            if let Some(target_task) = PROCESSOR
                .get_mut()
                .get_proc(ProcId::from_usize(pid as usize))
            {
                if let Ok(signal_no) = SignalNo::try_from(signum) {
                    if signal_no != SignalNo::ERR {
                        target_task.signal.add_signal(signal_no);
                        return 0;
                    }
                }
            }
            -1
        }

        fn sigaction(
            &self,
            _caller: Caller,
            signum: u8,
            action: usize,
            old_action: usize,
        ) -> isize {
            if signum as usize > tg_signal::MAX_SIG {
                return -1;
            }
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if let Ok(signal_no) = SignalNo::try_from(signum) {
                if signal_no == SignalNo::ERR {
                    return -1;
                }
                if old_action as usize != 0 {
                    if let Some(mut ptr) = current
                        .address_space
                        .translate(VAddr::new(old_action), WRITEABLE)
                    {
                        if let Some(signal_action) = current.signal.get_action_ref(signal_no) {
                            *unsafe { ptr.as_mut() } = signal_action;
                        } else {
                            return -1;
                        }
                    } else {
                        return -1;
                    }
                }
                if action as usize != 0 {
                    if let Some(ptr) = current
                        .address_space
                        .translate(VAddr::new(action), READABLE)
                    {
                        if !current
                            .signal
                            .set_action(signal_no, &unsafe { *ptr.as_ptr() })
                        {
                            return -1;
                        }
                    } else {
                        return -1;
                    }
                }
                return 0;
            }
            -1
        }

        fn sigprocmask(&self, _caller: Caller, mask: usize) -> isize {
            PROCESSOR
                .get_mut()
                .get_current_proc_for(current_hart_id())
                .unwrap()
                .signal
                .update_mask(mask) as isize
        }

        fn sigreturn(&self, _caller: Caller) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let ok = unsafe {
                (*processor)
                    .with_current_task_proc_for(current_hart_id(), |current_thread, current_proc| {
                        current_proc
                            .signal
                            .sig_return(&mut current_thread.context.context)
                    })
                    .unwrap()
            };
            if ok { 0 } else { -1 }
        }
    }

    /// 线程系统调用（**本章新增**）
    impl tg_syscall::Thread for SyscallContext {
        /// thread_create：在当前进程中创建新线程
        ///
        /// 为新线程分配独立的用户栈（从高地址向下搜索未映射的页面），
        /// 创建新的执行上下文，入口为 entry，参数为 arg。
        fn thread_create(&self, _caller: Caller, entry: usize, arg: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            // 分配 2 页用户栈
            let stack = unsafe {
                alloc_zeroed(Layout::from_size_align_unchecked(
                    2 << Sv39::PAGE_BITS,
                    1 << Sv39::PAGE_BITS,
                ))
            };
            let (pid, satp, user_sp) = unsafe {
                (*processor)
                    .with_current_proc_for(current_hart_id(), |current_proc| {
                        // 从最高用户栈位置向下搜索空闲的页表区域
                        let mut vpn = VPN::<Sv39>::new((1 << 26) - 2);
                        let addrspace = &mut current_proc.address_space;
                        loop {
                            let idx = vpn.index_in(Sv39::MAX_LEVEL);
                            if !addrspace.root()[idx].is_valid() {
                                break;
                            }
                            vpn = VPN::<Sv39>::new(vpn.val() - 3);
                        }
                        addrspace.map_extern(
                            vpn..vpn + 2,
                            PPN::new(stack as usize >> Sv39::PAGE_BITS),
                            build_flags("U_WRV"),
                        );
                        (
                            current_proc.pid,
                            (8 << 60) | addrspace.root_ppn().val(),
                            (vpn + 2).base().val(),
                        )
                    })
                    .unwrap()
            };
            let mut context = tg_kernel_context::LocalContext::user(entry);
            *context.sp_mut() = user_sp;
            *context.a_mut(0) = arg;
            let thread = Thread::new(satp, context);
            let tid = thread.tid;
            unsafe {
                (*processor).add_for(current_hart_id(), tid, Box::new(thread), pid);
            }
            tid.get_usize() as _
        }

        /// gettid：获取当前线程 TID
        fn gettid(&self, _caller: Caller) -> isize {
            PROCESSOR
                .get_mut()
                .current_tid_for(current_hart_id())
                .unwrap()
                .get_usize() as _
        }

        /// waittid：等待指定线程退出
        fn waittid(&self, _caller: Caller, tid: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let current_tid = unsafe { (*processor).current_tid_for(current_hart_id()).unwrap() };
            if tid == current_tid.get_usize() {
                return -1;
            }
            if let Some(exit_code) =
                unsafe { (*processor).waittid_for(current_hart_id(), ThreadId::from_usize(tid)) }
            {
                exit_code
            } else {
                -1
            }
        }
    }

    /// 同步原语系统调用（**本章新增**）
    ///
    /// 实现 Mutex、Semaphore、Condvar 的创建和操作。
    /// 这些同步原语存储在 Process 的列表中，由所有线程共享。
    impl SyncMutex for SyscallContext {
        /// 创建信号量（初始计数 = res_count）
        fn semaphore_create(&self, _caller: Caller, res_count: usize) -> isize {
            let current_proc = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            let id = if let Some(id) = current_proc
                .semaphore_list
                .iter()
                .enumerate()
                .find(|(_, item)| item.is_none())
                .map(|(id, _)| id)
            {
                current_proc.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));
                id
            } else {
                current_proc
                    .semaphore_list
                    .push(Some(Arc::new(Semaphore::new(res_count))));
                current_proc.semaphore_list.len() - 1
            };
            id as isize
        }

        /// V 操作：释放信号量，唤醒等待线程
        fn semaphore_up(&self, _caller: Caller, sem_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let (tid, sem) = unsafe {
                (*processor)
                    .with_current_task_proc_for(current_hart_id(), |current, current_proc| {
                        (
                            current.tid,
                            Arc::clone(current_proc.semaphore_list[sem_id].as_ref().unwrap()),
                        )
                    })
                    .unwrap()
            };
            if let Some(waking_tid) = sem.up(tid) {
                unsafe {
                    (*processor).re_enque_for(current_hart_id(), waking_tid);
                }
            }
            0
        }

        /// P 操作：获取信号量，不可用则阻塞
        fn semaphore_down(&self, _caller: Caller, sem_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let tid = unsafe { (*processor).current_tid_for(current_hart_id()).unwrap() };
            let (pid, deadlock_enabled, sem) = {
                let current_proc =
                    unsafe { (*processor).get_current_proc_for(current_hart_id()).unwrap() };
                (
                    current_proc.pid,
                    current_proc.deadlock_detect_enabled,
                    Arc::clone(current_proc.semaphore_list[sem_id].as_ref().unwrap()),
                )
            };
            if deadlock_enabled {
                let (avail, _, _) = sem.deadlock_snapshot();
                if avail == 0 && detect_semaphore_deadlock(processor, pid, tid, sem_id) {
                    return DEADLOCK_RET;
                }
            }
            if !sem.down(tid) { -1 } else { 0 }
        }

        /// 创建互斥锁（blocking=true 为阻塞锁）
        fn mutex_create(&self, _caller: Caller, blocking: bool) -> isize {
            let new_mutex: Option<Arc<dyn MutexTrait>> = if blocking {
                Some(Arc::new(MutexBlocking::new()))
            } else {
                Some(Arc::new(SpinLock::new()))
            };
            let current_proc = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if let Some(id) = current_proc
                .mutex_list
                .iter()
                .enumerate()
                .find(|(_, item)| item.is_none())
                .map(|(id, _)| id)
            {
                current_proc.mutex_list[id] = new_mutex;
                id as isize
            } else {
                current_proc.mutex_list.push(new_mutex);
                current_proc.mutex_list.len() as isize - 1
            }
        }

        /// 解锁，唤醒等待线程
        fn mutex_unlock(&self, _caller: Caller, mutex_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let mutex = unsafe {
                (*processor)
                    .with_current_proc_for(current_hart_id(), |current_proc| {
                        Arc::clone(current_proc.mutex_list[mutex_id].as_ref().unwrap())
                    })
                    .unwrap()
            };
            if let Some(tid) = mutex.unlock() {
                unsafe {
                    (*processor).re_enque_for(current_hart_id(), tid);
                }
            }
            0
        }

        /// 加锁，已被占用则阻塞
        fn mutex_lock(&self, _caller: Caller, mutex_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let tid = unsafe { (*processor).current_tid_for(current_hart_id()).unwrap() };
            let (pid, deadlock_enabled, mutex) = {
                let current_proc =
                    unsafe { (*processor).get_current_proc_for(current_hart_id()).unwrap() };
                (
                    current_proc.pid,
                    current_proc.deadlock_detect_enabled,
                    Arc::clone(current_proc.mutex_list[mutex_id].as_ref().unwrap()),
                )
            };
            if deadlock_enabled
                && mutex.should_block_on_fail()
                && mutex.holder().is_some()
                && detect_mutex_deadlock(processor, pid, tid, mutex_id)
            {
                return DEADLOCK_RET;
            }
            if mutex.lock(tid) {
                0
            } else if mutex.should_block_on_fail() {
                -1
            } else {
                -2
            }
        }

        /// 创建条件变量
        fn condvar_create(&self, _caller: Caller, _arg: usize) -> isize {
            let current_proc = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            let id = if let Some(id) = current_proc
                .condvar_list
                .iter()
                .enumerate()
                .find(|(_, item)| item.is_none())
                .map(|(id, _)| id)
            {
                current_proc.condvar_list[id] = Some(Arc::new(Condvar::new()));
                id
            } else {
                current_proc
                    .condvar_list
                    .push(Some(Arc::new(Condvar::new())));
                current_proc.condvar_list.len() - 1
            };
            id as isize
        }

        /// 唤醒一个等待线程
        fn condvar_signal(&self, _caller: Caller, condvar_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let condvar = unsafe {
                (*processor)
                    .with_current_proc_for(current_hart_id(), |current_proc| {
                        Arc::clone(current_proc.condvar_list[condvar_id].as_ref().unwrap())
                    })
                    .unwrap()
            };
            if let Some(tid) = condvar.signal() {
                unsafe {
                    (*processor).re_enque_for(current_hart_id(), tid);
                }
            }
            0
        }

        /// 等待条件变量（释放锁 + 阻塞 + 重新获取锁）
        fn condvar_wait(&self, _caller: Caller, condvar_id: usize, mutex_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let (tid, condvar, mutex) = unsafe {
                (*processor)
                    .with_current_task_proc_for(current_hart_id(), |current, current_proc| {
                        (
                            current.tid,
                            Arc::clone(current_proc.condvar_list[condvar_id].as_ref().unwrap()),
                            Arc::clone(current_proc.mutex_list[mutex_id].as_ref().unwrap()),
                        )
                    })
                    .unwrap()
            };
            let (flag, waking_tid) = condvar.wait_with_mutex(tid, mutex);
            if let Some(waking_tid) = waking_tid {
                unsafe {
                    (*processor).re_enque_for(current_hart_id(), waking_tid);
                }
            }
            if !flag { -1 } else { 0 }
        }

        /// 创建读写锁
        fn rwlock_create(&self, _caller: Caller) -> isize {
            let current_proc = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            let id = if let Some(id) = current_proc
                .rwlock_list
                .iter()
                .enumerate()
                .find(|(_, item)| item.is_none())
                .map(|(id, _)| id)
            {
                current_proc.rwlock_list[id] = Some(Arc::new(RwLock::new()));
                id
            } else {
                current_proc.rwlock_list.push(Some(Arc::new(RwLock::new())));
                current_proc.rwlock_list.len() - 1
            };
            id as isize
        }

        /// 获取读锁，不可用则阻塞
        fn rwlock_read_lock(&self, _caller: Caller, rwlock_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let (tid, rwlock) = unsafe {
                (*processor)
                    .with_current_task_proc_for(current_hart_id(), |current, current_proc| {
                        (
                            current.tid,
                            Arc::clone(current_proc.rwlock_list[rwlock_id].as_ref().unwrap()),
                        )
                    })
                    .unwrap()
            };
            if rwlock.try_read(tid) { 0 } else { -1 }
        }

        /// 获取写锁，不可用则阻塞
        fn rwlock_write_lock(&self, _caller: Caller, rwlock_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let (tid, rwlock) = unsafe {
                (*processor)
                    .with_current_task_proc_for(current_hart_id(), |current, current_proc| {
                        (
                            current.tid,
                            Arc::clone(current_proc.rwlock_list[rwlock_id].as_ref().unwrap()),
                        )
                    })
                    .unwrap()
            };
            if rwlock.try_write(tid) { 0 } else { -1 }
        }

        /// 释放读写锁，并唤醒等待线程
        fn rwlock_unlock(&self, _caller: Caller, rwlock_id: usize) -> isize {
            let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
            let (tid, rwlock) = unsafe {
                (*processor)
                    .with_current_task_proc_for(current_hart_id(), |current, current_proc| {
                        (
                            current.tid,
                            Arc::clone(current_proc.rwlock_list[rwlock_id].as_ref().unwrap()),
                        )
                    })
                    .unwrap()
            };
            for waking_tid in rwlock.unlock_for(tid) {
                unsafe {
                    (*processor).re_enque_for(current_hart_id(), waking_tid);
                }
            }
            0
        }

        /// 死锁检测
        fn enable_deadlock_detect(&self, _caller: Caller, is_enable: i32) -> isize {
            if is_enable != 0 && is_enable != 1 {
                return -1;
            }
            let current_proc = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            current_proc.deadlock_detect_enabled = is_enable == 1;
            0
        }
    }

    /// 帧缓冲系统调用：查询分辨率、提交 doomgeneric 软件帧。
    impl Framebuffer for SyscallContext {
        fn fb_get_info(&self, _caller: Caller, info_ptr: usize) -> isize {
            let Some((w, h, stride)) = crate::virtio_gpu::dimensions() else {
                return -1;
            };
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            if let Some(mut ptr) = current
                .address_space
                .translate(VAddr::new(info_ptr), WRITEABLE)
            {
                unsafe {
                    let base = ptr.as_mut() as *mut u32;
                    base.write_unaligned(w);
                    base.add(1).write_unaligned(h);
                    base.add(2).write_unaligned(stride);
                }
                0
            } else {
                -1
            }
        }

        fn fb_present(&self, _caller: Caller, buf: usize, len: usize) -> isize {
            let need = crate::virtio_gpu::DOOM_FB_BYTES;
            if len < need {
                log::warn!("fb_present: short user buffer, len={len}, need={need}");
                return -1;
            }
            let current = PROCESSOR.get_mut().get_current_proc_for(current_hart_id()).unwrap();
            let mut frame = FB_UPLOAD_BUF.lock();
            if frame.len() < need {
                frame.resize(need, 0);
            }
            let frame = &mut frame[..need];
            if !copy_from_user(&current.address_space, buf, frame) {
                log::warn!("fb_present: invalid user buffer addr={:#x} len={need}", buf);
                return -1;
            }
            match crate::virtio_gpu::present(frame) {
                Ok(()) => 0,
                Err(()) => {
                    log::warn!("fb_present: virtio-gpu present failed");
                    -1
                }
            }
        }
    }
}

/// 非 RISC-V64 架构的占位实现
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    use tg_kernel_vm::page_table::{MmuMeta, VmFlags};

    /// Sv39 占位类型
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
    pub struct Sv39;
    impl MmuMeta for Sv39 {
        const P_ADDR_BITS: usize = 56;
        const PAGE_BITS: usize = 12;
        const LEVEL_BITS: &'static [usize] = &[9, 9, 9];
        const PPN_POS: usize = 10;
        #[inline]
        fn is_leaf(value: usize) -> bool {
            value & 0b1110 != 0
        }
    }
    /// 构建 VmFlags 占位
    pub const fn build_flags(_s: &str) -> VmFlags<Sv39> {
        unsafe { VmFlags::from_raw(0) }
    }
    /// 解析 VmFlags 占位
    pub fn parse_flags(_s: &str) -> Result<VmFlags<Sv39>, ()> {
        Ok(unsafe { VmFlags::from_raw(0) })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 {
        0
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 {
        0
    }
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}
