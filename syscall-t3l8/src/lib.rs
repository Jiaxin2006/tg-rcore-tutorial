#![no_std]
#![deny(warnings)]
//!
//! 教程阅读建议：
//!
//! - 用户态读 `user.rs`：看 syscall 封装如何把参数放入 a0-a5/a7；
//! - 内核态读 `kernel/mod.rs`：看 syscall 号如何分发到各子系统 trait。

#[cfg(all(feature = "kernel", feature = "user"))]
compile_error!("You can only use one of `supervisor` or `user` features at a time");

mod fs;
mod io;
mod time;

include!(concat!(env!("OUT_DIR"), "/syscalls.rs"));
// 由构建脚本生成的 syscall 编号常量（与课程章节保持同步）。

pub use fs::*;
pub use io::*;
pub use tg_signal_defs::{MAX_SIG, SignalAction, SignalNo};
pub use time::*;

#[cfg(feature = "user")]
mod user;

#[cfg(feature = "user")]
pub use user::*;

#[cfg(feature = "kernel")]
mod kernel;

#[cfg(feature = "kernel")]
pub use kernel::*;

/// 系统调用号。
///
/// 实现为包装类型，在不损失扩展性的情况下实现类型安全性。
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[repr(transparent)]
pub struct SyscallId(pub usize);

impl From<usize> for SyscallId {
    #[inline]
    fn from(val: usize) -> Self {
        Self(val)
    }
}

/// 当前 SMP 调试接口一次最多暴露的 hart 数量。
pub const SMP_HART_CAPACITY: usize = 8;

/// 内核导出的多核本地状态快照。
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct HartSnapshot {
    /// 内核编译时支持的最大 hart 数。
    pub max_harts: usize,
    /// 当前已经完成 bring-up 的 hart 数。
    pub online_harts: usize,
    /// online hart 的位图，bit i 表示 hart i 已上线。
    pub online_mask: usize,
    /// 发起这次 syscall 的 hart id。
    pub current_hart: usize,
    /// 当前被置位 `need_resched` 的 hart 位图。
    pub need_resched_mask: usize,
    /// 每个 hart 的逻辑 tick 计数。
    pub timer_ticks: [u64; SMP_HART_CAPACITY],
    /// 每个 hart 在 S 态内核代码里实际收到的 timer interrupt 次数。
    pub kernel_timer_interrupts: [u64; SMP_HART_CAPACITY],
}
