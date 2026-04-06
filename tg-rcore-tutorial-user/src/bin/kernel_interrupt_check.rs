#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{get_time, kernel_interrupt_check};

// 教学目标：
// 用一条专门的长 syscall 自证“内核态确实收到了 timer interrupt”。

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    const REQUIRED_INTERRUPTS: usize = 2;

    let start = get_time();
    let observed = kernel_interrupt_check(REQUIRED_INTERRUPTS);
    let elapsed = get_time() - start;

    println!(
        "kernel interrupt check: observed {} kernel timer interrupts during one syscall ({} ms)",
        observed, elapsed,
    );
    if observed < REQUIRED_INTERRUPTS as isize {
        println!("kernel interrupt check failed!");
        return 1;
    }
    println!("kernel interrupt check passed!");
    0
}
