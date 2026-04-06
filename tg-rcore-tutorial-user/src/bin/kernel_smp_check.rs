#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use user_lib::{
    HartSnapshot, exit, kernel_hart_snapshot, sched_yield, sleep, thread_create, waittid,
};

static START_WORKERS: AtomicBool = AtomicBool::new(false);
static USER_OBSERVED_HART_MASK: AtomicUsize = AtomicUsize::new(0);

const WORKER_COUNT: usize = 8;
const REQUIRED_HART_MASK: usize = 0x3;

fn smp_worker(_arg: usize) -> isize {
    while !START_WORKERS.load(Ordering::Acquire) {
        sched_yield();
    }

    for _ in 0..256 {
        let mut snapshot = HartSnapshot::default();
        let ret = kernel_hart_snapshot(&mut snapshot);
        if ret < 0 {
            println!("kernel smp worker snapshot failed: ret={}", ret);
            exit(1);
        }
        USER_OBSERVED_HART_MASK.fetch_or(1usize << snapshot.current_hart, Ordering::SeqCst);
        sched_yield();
    }

    exit(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut before = HartSnapshot::default();
    if kernel_hart_snapshot(&mut before) < 0 {
        println!("kernel smp check failed: first snapshot unavailable");
        return 1;
    }

    sleep(100);

    let mut after = HartSnapshot::default();
    if kernel_hart_snapshot(&mut after) < 0 {
        println!("kernel smp check failed: second snapshot unavailable");
        return 1;
    }

    let hart0_delta = after.timer_ticks[0].saturating_sub(before.timer_ticks[0]);
    let hart1_delta = after.timer_ticks[1].saturating_sub(before.timer_ticks[1]);
    let hart1_kernel_delta =
        after.kernel_timer_interrupts[1].saturating_sub(before.kernel_timer_interrupts[1]);

    println!("online_mask=0x{:x}", after.online_mask);
    println!("hart0 delta: ticks={}", hart0_delta);
    println!(
        "hart1 delta: ticks={} kernel_timer_interrupts={}",
        hart1_delta, hart1_kernel_delta,
    );

    if (after.online_mask & REQUIRED_HART_MASK) != REQUIRED_HART_MASK
        || hart0_delta == 0
        || hart1_delta == 0
    {
        println!("kernel smp check failed!");
        return 1;
    }

    USER_OBSERVED_HART_MASK.store(0, Ordering::SeqCst);
    START_WORKERS.store(false, Ordering::SeqCst);

    let mut tids = [0isize; WORKER_COUNT];
    for (i, tid_slot) in tids.iter_mut().enumerate() {
        let tid = thread_create(smp_worker as *const () as usize, i);
        if tid < 0 {
            println!("kernel smp check failed: thread_create ret={}", tid);
            return 1;
        }
        *tid_slot = tid;
    }

    START_WORKERS.store(true, Ordering::Release);

    for &tid in &tids {
        let exit_code = waittid(tid as usize);
        if exit_code != 0 {
            println!(
                "kernel smp check failed: worker tid={} exit_code={}",
                tid, exit_code,
            );
            return 1;
        }
    }

    let user_mask = USER_OBSERVED_HART_MASK.load(Ordering::SeqCst);
    println!("user observed hart mask=0x{:x}", user_mask);
    if (user_mask & REQUIRED_HART_MASK) != REQUIRED_HART_MASK {
        println!("kernel smp check failed!");
        return 1;
    }

    println!("kernel smp check passed!");
    0
}
