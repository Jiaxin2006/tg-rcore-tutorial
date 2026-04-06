#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::sync::atomic::{AtomicUsize, Ordering};
use user_lib::{
    ClockId, FbInfo, HartSnapshot, TimeSpec, clock_gettime, exit, fb_get_info, fb_present,
    kernel_hart_snapshot, sched_yield, thread_create, waittid,
};

const W: usize = 640;
const H: usize = 400;
const BYTES: usize = W * H * 4;
const TILE_W: usize = 32;
const TILE_H: usize = 25;
const TILE_COLS: usize = W / TILE_W;
const TILE_ROWS: usize = H / TILE_H;
const TOTAL_TILES: usize = TILE_COLS * TILE_ROWS;
const PASSES: usize = 4;
const MAX_ITER: usize = 96;
const BENCH_ROUNDS: usize = 2;
const MAX_WORKERS: usize = 8;
const REQUIRED_ALPHA: u8 = 0xff;
const ESCAPE_RADIUS_SQUARED: i64 = 4 << FP_SHIFT;
const FP_SHIFT: i64 = 16;
const FP_ONE: i64 = 1 << FP_SHIFT;

static USER_OBSERVED_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(1);

static mut FRAME: [u8; BYTES] = [0; BYTES];

struct BenchSummary {
    avg_us: u64,
    best_us: u64,
    checksum: u64,
    observed_harts: usize,
}

#[inline]
fn now_us() -> u64 {
    let mut ts = TimeSpec::ZERO;
    clock_gettime(ClockId::CLOCK_MONOTONIC, &mut ts as *mut _ as _);
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000
}

#[inline]
fn frame_ptr() -> *mut u8 {
    core::ptr::addr_of_mut!(FRAME).cast::<u8>()
}

#[inline]
fn frame_slice() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(FRAME).cast::<u8>(), BYTES) }
}

#[inline]
fn write_pixel(x: usize, y: usize, rgba: [u8; 4]) {
    let idx = (y * W + x) * 4;
    let base = frame_ptr();
    unsafe {
        base.add(idx).write(rgba[0]);
        base.add(idx + 1).write(rgba[1]);
        base.add(idx + 2).write(rgba[2]);
        base.add(idx + 3).write(rgba[3]);
    }
}

#[inline]
fn record_current_hart() {
    let mut snapshot = HartSnapshot::default();
    if kernel_hart_snapshot(&mut snapshot) >= 0 {
        USER_OBSERVED_HART_MASK.fetch_or(1usize << snapshot.current_hart, Ordering::SeqCst);
    }
}

fn pixel_color(x: usize, y: usize, pass: usize) -> [u8; 4] {
    let pass = pass as i64;
    let x_min = (-2300 + pass * 140) * FP_ONE / 1000;
    let y_min = (-1250 + pass * 90) * FP_ONE / 1000;
    let x_span = (3200 - pass * 220) * FP_ONE / 1000;
    let y_span = (2500 - pass * 170) * FP_ONE / 1000;

    let cx = x_min + (x as i64 * x_span) / W as i64;
    let cy = y_min + (y as i64 * y_span) / H as i64;

    let mut zx = 0i64;
    let mut zy = 0i64;
    let mut iter = 0usize;

    while iter < MAX_ITER {
        let zx2 = (zx * zx) >> FP_SHIFT;
        let zy2 = (zy * zy) >> FP_SHIFT;
        if zx2 + zy2 > ESCAPE_RADIUS_SQUARED {
            break;
        }
        let zxy = (zx * zy) >> (FP_SHIFT - 1);
        zx = zx2 - zy2 + cx;
        zy = zxy + cy;
        iter += 1;
    }

    if iter == MAX_ITER {
        return [8, 12, 20, REQUIRED_ALPHA];
    }

    let trap = ((zx.abs() + zy.abs()) >> 6) as usize;
    let r = ((iter * 9 + trap + pass as usize * 23) & 0xff) as u8;
    let g = ((iter * 5 + (x / 3) + pass as usize * 17) & 0xff) as u8;
    let b = ((iter * 13 + (y / 2) + trap / 3) & 0xff) as u8;
    [r, g, b, REQUIRED_ALPHA]
}

fn render_tile(tile_id: usize, pass: usize) {
    let tx = tile_id % TILE_COLS;
    let ty = tile_id / TILE_COLS;
    let x0 = tx * TILE_W;
    let y0 = ty * TILE_H;
    for y in y0..(y0 + TILE_H) {
        for x in x0..(x0 + TILE_W) {
            write_pixel(x, y, pixel_color(x, y, pass));
        }
    }
}

fn render_scene(worker_id: usize, worker_count: usize) {
    for pass in 0..PASSES {
        record_current_hart();
        for tile_id in (worker_id..TOTAL_TILES).step_by(worker_count) {
            render_tile(tile_id, pass);
        }
        sched_yield();
    }
    record_current_hart();
}

fn worker_main(arg: usize) -> isize {
    let worker_count = WORKER_COUNT.load(Ordering::Acquire).max(1);
    render_scene(arg, worker_count);
    exit(0)
}

#[inline]
fn checksum_frame() -> u64 {
    let mut acc = 0xcbf2_9ce4_8422_2325u64;
    for &byte in frame_slice() {
        acc ^= byte as u64;
        acc = acc.wrapping_mul(0x100_0000_01b3);
    }
    acc
}

fn measure_workers_once(worker_count: usize) -> (u64, u64, usize) {
    WORKER_COUNT.store(worker_count.max(1), Ordering::Release);
    USER_OBSERVED_HART_MASK.store(0, Ordering::SeqCst);

    let spawn_count = worker_count.saturating_sub(1).min(MAX_WORKERS - 1);
    let mut tids = [0isize; MAX_WORKERS - 1];
    let start = now_us();

    for (idx, tid_slot) in tids.iter_mut().take(spawn_count).enumerate() {
        let worker_id = idx + 1;
        let tid = thread_create(worker_main as *const () as usize, worker_id);
        if tid < 0 {
            println!("smp bench: thread_create failed for worker {}", worker_id);
            return (0, 0, 0);
        }
        *tid_slot = tid;
    }

    render_scene(0, worker_count.max(1));

    for &tid in tids.iter().take(spawn_count) {
        let exit_code = waittid(tid as usize);
        if exit_code != 0 {
            println!("smp bench: worker tid={} exit_code={}", tid, exit_code);
            return (0, 0, 0);
        }
    }

    let elapsed = now_us() - start;
    (
        elapsed,
        checksum_frame(),
        USER_OBSERVED_HART_MASK.load(Ordering::SeqCst),
    )
}

fn measure_workers(worker_count: usize) -> Option<BenchSummary> {
    let mut total_us = 0u64;
    let mut best_us = u64::MAX;
    let mut checksum = 0u64;
    let mut observed_harts = 0usize;

    for round in 0..BENCH_ROUNDS {
        let (elapsed, round_checksum, round_harts) = measure_workers_once(worker_count);
        if elapsed == 0 {
            return None;
        }
        if round == 0 {
            checksum = round_checksum;
        } else if checksum != round_checksum {
            println!(
                "smp bench: checksum mismatch between rounds: prev={:#x} now={:#x}",
                checksum, round_checksum
            );
            return None;
        }
        total_us += elapsed;
        best_us = best_us.min(elapsed);
        observed_harts |= round_harts;
        println!(
            "workers={} round={} elapsed={} us checksum={:#x} observed_harts={:#x}",
            worker_count,
            round + 1,
            elapsed,
            round_checksum,
            round_harts
        );
    }

    Some(BenchSummary {
        avg_us: total_us / BENCH_ROUNDS as u64,
        best_us,
        checksum,
        observed_harts,
    })
}

#[inline]
fn present_frame_if_possible() {
    let mut info = FbInfo::default();
    if fb_get_info(&mut info as *mut _) >= 0 {
        let _ = fb_present(frame_slice().as_ptr(), BYTES);
    }
}

#[inline]
fn print_speedup(single_us: u64, multi_us: u64) {
    if multi_us == 0 {
        println!("speedup: invalid");
        return;
    }
    let scaled = single_us.saturating_mul(100) / multi_us;
    println!("speedup (1 worker -> N workers): {}.{:02}x", scaled / 100, scaled % 100);
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut snapshot = HartSnapshot::default();
    let online_harts = if kernel_hart_snapshot(&mut snapshot) >= 0 {
        snapshot.online_harts.max(1).min(MAX_WORKERS)
    } else {
        1
    };

    println!(
        "smp bench: render={}x{} tiles={} online_harts={} rounds={} passes={} max_iter={}",
        W, H, TOTAL_TILES, online_harts, BENCH_ROUNDS, PASSES, MAX_ITER
    );
    println!("smp bench: compare 1 worker against {} workers", online_harts);

    let single = match measure_workers(1) {
        Some(summary) => summary,
        None => {
            println!("smp bench failed!");
            return 1;
        }
    };
    let multi = match measure_workers(online_harts) {
        Some(summary) => summary,
        None => {
            println!("smp bench failed!");
            return 1;
        }
    };

    println!(
        "single-worker avg={} us best={} us checksum={:#x}",
        single.avg_us, single.best_us, single.checksum
    );
    println!(
        "multi-worker  avg={} us best={} us checksum={:#x} observed_harts={:#x}",
        multi.avg_us, multi.best_us, multi.checksum, multi.observed_harts
    );
    print_speedup(single.avg_us, multi.avg_us);

    if single.checksum != multi.checksum {
        println!("smp bench failed: single/multi checksum mismatch");
        return 1;
    }

    if online_harts > 1 && multi.observed_harts.count_ones() < 2 {
        println!("smp bench failed: worker set did not spread across multiple harts");
        return 1;
    }

    if online_harts == 1 {
        println!("smp bench note: only one hart online, so this run is a single-core baseline.");
    }

    present_frame_if_possible();
    println!("smp bench passed!");
    0
}
