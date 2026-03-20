//! 任务管理模块
//!
//! 定义了任务控制块（Task Control Block, TCB）和调度事件，
//! 是多道程序系统的核心数据结构。
//!
//! ## 与第二章的区别
//!
//! 第二章的批处理系统中，用户态上下文直接在 `rust_main` 的局部变量中管理。
//! 本章将其封装到 `TaskControlBlock` 中，每个任务拥有独立的 TCB，
//! 包含用户上下文、完成状态和独立的用户栈，支持多任务并发。
//!
//! 教程阅读建议：
//!
//! - 先看 `TaskControlBlock` 字段：理解“上下文 + 栈 + 状态位”最小任务模型；
//! - 再看 `handle_syscall`：理解系统调用结果如何映射成调度事件；
//! - 最后对照 `ch3/src/main.rs`：把“事件生成”和“事件消费”串成闭环。

use tg_kernel_context::LocalContext;
use tg_syscall::{Caller, SyscallId};

/// 与 `main.rs` 中 `APP_CAPACITY` 一致：内核可同时容纳的用户任务槽位数。
pub const MAX_APP_TASKS: usize = 32;

#[cfg(feature = "exercise")]
mod syscall_trace {
    //! 每任务一张**稀疏表**：只记录实际出现过的系统调用号及其次数，不占 `512×usize` 稠密表。
    //! 表放在 BSS，不增大 `rust_main` 栈上的 `tcbs` 数组。

    /// 单任务最多记录多少种不同的系统调用号（同一号多次调用只占一条）。
    const MAX_DISTINCT: usize = 64;

    #[derive(Clone, Copy)]
    struct Entry {
        nr: u32,
        count: u32,
    }

    #[derive(Clone, Copy)]
    struct TaskTable {
        len: u8,
        entries: [Entry; MAX_DISTINCT],
    }

    impl TaskTable {
        const fn empty() -> Self {
            Self {
                len: 0,
                entries: [Entry { nr: 0, count: 0 }; MAX_DISTINCT],
            }
        }
    }

    static mut TABLES: [TaskTable; super::MAX_APP_TASKS] =
        [TaskTable::empty(); super::MAX_APP_TASKS];

    pub(super) fn clear_row(task: usize) {
        if task < super::MAX_APP_TASKS {
            unsafe {
                TABLES[task].len = 0;
            }
        }
    }

    pub(super) fn bump(task: usize, syscall_id: usize) {
        if task >= super::MAX_APP_TASKS {
            return;
        }
        let nr = syscall_id as u32;
        unsafe {
            let t = &mut TABLES[task];
            let n = t.len as usize;
            for i in 0..n {
                if t.entries[i].nr == nr {
                    t.entries[i].count = t.entries[i].count.saturating_add(1);
                    return;
                }
            }
            if n < MAX_DISTINCT {
                t.entries[n] = Entry { nr, count: 1 };
                t.len += 1;
            }
        }
    }

    pub(super) fn get(task: usize, syscall_id: usize) -> usize {
        if task >= super::MAX_APP_TASKS {
            return 0;
        }
        let nr = syscall_id as u32;
        unsafe {
            let t = &TABLES[task];
            let n = t.len as usize;
            for i in 0..n {
                if t.entries[i].nr == nr {
                    return t.entries[i].count as usize;
                }
            }
        }
        0
    }
}

/// 任务控制块（Task Control Block, TCB）
///
/// 每个用户程序对应一个 TCB，包含：
/// - `ctx`：用户态上下文（所有通用寄存器 + 控制寄存器），用于任务切换时保存/恢复状态
/// - `finish`：任务是否已完成（退出或被杀死）
/// - `stack`：用户栈空间（8 KiB），每个任务有独立的栈
pub struct TaskControlBlock {
    /// 用户态上下文：保存 Trap 时的所有寄存器状态
    ctx: LocalContext,
    /// 任务完成标志：true 表示已退出或被杀死
    pub finish: bool,
    /// 用户栈：8 KiB（1024 个 usize = 1024 × 8 = 8192 字节）
    /// 每个任务拥有独立的栈空间，避免栈溢出影响其他任务
    stack: [usize; 1024],
}

/// 调度事件
///
/// `handle_syscall` 处理完系统调用后返回此枚举，
/// 告知主循环应该如何调度当前任务。
pub enum SchedulingEvent {
    /// 系统调用处理完成，继续执行当前任务（如 write、clock_gettime）
    None,
    /// 任务主动让出 CPU（yield 系统调用），切换到下一个任务
    Yield,
    /// 任务请求退出（exit 系统调用），附带退出码
    Exit(usize),
    /// 不支持的系统调用，附带系统调用 ID
    UnsupportedSyscall(SyscallId),
}

impl TaskControlBlock {
    /// 零值常量：用于数组初始化
    pub const ZERO: Self = Self {
        ctx: LocalContext::empty(),
        finish: false,
        stack: [0; 1024],
    };

    /// 初始化一个任务
    ///
    /// - 清零用户栈
    /// - 创建用户态上下文，设置入口地址和 `sstatus.SPP = User`
    /// - 将栈指针设置为用户栈的栈顶（高地址端）
    /// - `task_index`：当前任务在 `tcbs` 中的下标（练习模式下用于 `sys_trace` 统计表）
    pub fn init(&mut self, entry: usize, task_index: usize) {
        let _ = task_index;
        self.stack.fill(0);
        #[cfg(feature = "exercise")]
        syscall_trace::clear_row(task_index);
        self.finish = false;
        self.ctx = LocalContext::user(entry);
        // 栈从高地址向低地址增长，所以 sp 指向栈顶（数组末尾之后的地址）
        *self.ctx.sp_mut() = self.stack.as_ptr() as usize + core::mem::size_of_val(&self.stack);
    }

    /// 执行此任务
    ///
    /// 恢复用户寄存器并执行 `sret` 切换到 U-mode。
    /// 当用户程序触发 Trap 后返回到此函数的调用处。
    #[inline]
    pub unsafe fn execute(&mut self) {
        unsafe { self.ctx.execute() };
    }

    /// 处理系统调用，返回调度事件
    ///
    /// 从用户上下文中提取系统调用 ID（a7 寄存器）和参数（a0-a5 寄存器），
    /// 分发到对应的处理函数，并将返回值写回 a0 寄存器。
    ///
    /// `task_index`：当前任务在 `tcbs` 中的下标（与 `init` 时一致）。
    pub fn handle_syscall(&mut self, task_index: usize) -> SchedulingEvent {
        let _ = task_index;
        use tg_syscall::{SyscallId as Id, SyscallResult as Ret};
        use SchedulingEvent as Event;

        // a7 寄存器存放 syscall ID
        let id: SyscallId = self.ctx.a(7).into();
        // a0-a5 寄存器存放系统调用参数
        let args = [
            self.ctx.a(0),
            self.ctx.a(1),
            self.ctx.a(2),
            self.ctx.a(3),
            self.ctx.a(4),
            self.ctx.a(5),
        ];

        #[cfg(feature = "exercise")]
        {
            let id_idx = id.0;
            syscall_trace::bump(task_index, id_idx);

            if id == Id::TRACE {
                let ret = match args[0] {
                    0 => (unsafe { *(args[1] as *const u8) }) as isize,
                    1 => {
                        unsafe { *(args[1] as *mut u8) = args[2] as u8 };
                        0
                    }
                    2 => syscall_trace::get(task_index, args[1]) as isize,
                    _ => -1,
                };
                *self.ctx.a_mut(0) = ret as _;
                self.ctx.move_next();
                return Event::None;
            }
        }

        match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
            Ret::Done(ret) => match id {
                // exit 系统调用：返回退出事件
                Id::EXIT => Event::Exit(self.ctx.a(0)),
                // yield 系统调用：返回让出事件
                Id::SCHED_YIELD => {
                    *self.ctx.a_mut(0) = ret as _;
                    self.ctx.move_next(); // sepc += 4，跳过 ecall 指令
                    Event::Yield
                }
                // 其他系统调用（如 write、clock_gettime）：继续执行
                _ => {
                    *self.ctx.a_mut(0) = ret as _;
                    self.ctx.move_next(); // sepc += 4，跳过 ecall 指令
                    Event::None
                }
            },
            // 不支持的系统调用
            Ret::Unsupported(_) => Event::UnsupportedSyscall(id),
        }
    }
}
