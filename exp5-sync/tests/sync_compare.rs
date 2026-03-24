//! 同步原语集成测试
//!
//! 包含：
//! - 正向功能测试（每个原语的基本正确性）
//! - 反向测试（故意缺少 unlock / 错误顺序 -> 检测死锁/超时）
//! - 多线程压力测试
//! - 对比测试（spin vs sleep / rwlock vs mutex）

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use jiaxin2006_tg_rcore_tutorial_t2l5::metrics::InstrumentedMutex;
use jiaxin2006_tg_rcore_tutorial_t2l5::primitives::*;
use jiaxin2006_tg_rcore_tutorial_t2l5::scenarios::*;
use jiaxin2006_tg_rcore_tutorial_t2l5::workload::*;

// ===========================================================================
// 正向功能测试
// ===========================================================================

#[test]
fn test_spinlock_basic() {
    let lock = SpinLock::new();
    assert!(lock.lock(1));
    assert_eq!(lock.holder(), Some(1));
    assert!(!lock.lock(2));
    assert_eq!(lock.waiting(), vec![2]);

    let woken = lock.unlock();
    assert_eq!(woken, Some(2));
    assert_eq!(lock.holder(), Some(2));

    let woken = lock.unlock();
    assert_eq!(woken, None);
    assert_eq!(lock.holder(), None);
}

#[test]
fn test_mutex_blocking_basic() {
    let mutex = MutexBlocking::new();
    assert!(mutex.lock(1));
    assert_eq!(mutex.holder(), Some(1));
    assert!(!mutex.lock(2));
    assert!(!mutex.lock(3));
    assert_eq!(mutex.waiting(), vec![2, 3]);

    let woken = mutex.unlock();
    assert_eq!(woken, Some(2));
    assert_eq!(mutex.holder(), Some(2));

    let woken = mutex.unlock();
    assert_eq!(woken, Some(3));

    let woken = mutex.unlock();
    assert_eq!(woken, None);
}

#[test]
fn test_semaphore_basic() {
    let sem = Semaphore::new(2);
    assert!(sem.down(1));
    assert!(sem.down(2));
    assert!(!sem.down(3));
    assert_eq!(sem.count(), -1);
    assert_eq!(sem.waiting_count(), 1);

    let woken = sem.up(1);
    assert_eq!(woken, Some(3));
    assert_eq!(sem.count(), 0);
}

#[test]
fn test_condvar_basic() {
    let cv = Condvar::new();
    assert!(!cv.wait_no_sched(1));
    assert!(!cv.wait_no_sched(2));
    assert_eq!(cv.waiting_count(), 2);

    let woken = cv.signal();
    assert_eq!(woken, Some(1));
    assert_eq!(cv.waiting_count(), 1);

    let woken = cv.signal();
    assert_eq!(woken, Some(2));
    assert_eq!(cv.waiting_count(), 0);

    let woken = cv.signal();
    assert_eq!(woken, None);
}

#[test]
fn test_rwlock_basic() {
    let rw = RwLock::new();

    rw.read_lock(1);
    rw.read_lock(2);
    assert_eq!(rw.reader_count(), 2);
    assert!(!rw.writer_active());

    rw.read_unlock();
    rw.read_unlock();
    assert_eq!(rw.reader_count(), 0);

    rw.write_lock(3);
    assert!(rw.writer_active());
    rw.write_unlock();
    assert!(!rw.writer_active());
}

// ===========================================================================
// 多线程正确性测试
// ===========================================================================

#[test]
fn test_mutex_blocking_concurrent() {
    let mutex = Arc::new(MutexBlocking::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let n_threads = 8;
    let n_iterations = 500;

    let handles: Vec<_> = (0..n_threads)
        .map(|tid| {
            let mutex = Arc::clone(&mutex);
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..n_iterations {
                    mutex.acquire(tid + 1);
                    counter.fetch_add(1, Ordering::Relaxed);
                    mutex.release();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        counter.load(Ordering::Relaxed),
        n_threads * n_iterations
    );
}

#[test]
fn test_spinlock_concurrent() {
    let lock = Arc::new(SpinLock::new());
    let counter = Arc::new(AtomicUsize::new(0));
    let n_threads = 8;
    let n_iterations = 500;

    let handles: Vec<_> = (0..n_threads)
        .map(|tid| {
            let lock = Arc::clone(&lock);
            let counter = Arc::clone(&counter);
            thread::spawn(move || {
                for _ in 0..n_iterations {
                    lock.acquire(tid + 1);
                    counter.fetch_add(1, Ordering::Relaxed);
                    lock.release();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        counter.load(Ordering::Relaxed),
        n_threads * n_iterations
    );
}

#[test]
fn test_semaphore_concurrent() {
    let sem = Arc::new(Semaphore::new(3));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let n_threads = 10;

    let handles: Vec<_> = (0..n_threads)
        .map(|tid| {
            let sem = Arc::clone(&sem);
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            thread::spawn(move || {
                for _ in 0..20 {
                    sem.acquire(tid + 1);
                    let current = active.fetch_add(1, Ordering::Relaxed) + 1;
                    max_active.fetch_max(current, Ordering::Relaxed);
                    thread::yield_now();
                    active.fetch_sub(1, Ordering::Relaxed);
                    sem.release(tid + 1);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert!(
        max_active.load(Ordering::Relaxed) <= 3,
        "Semaphore allowed more than 3 concurrent accesses"
    );
}

#[test]
fn test_rwlock_concurrent() {
    let rwlock = Arc::new(RwLock::new());
    let shared = Arc::new(AtomicUsize::new(0));
    let active_readers = Arc::new(AtomicUsize::new(0));
    let n_readers = 6;
    let n_writers = 2;

    let mut handles = Vec::new();

    for rid in 0..n_readers {
        let rwlock = Arc::clone(&rwlock);
        let shared = Arc::clone(&shared);
        let active_readers = Arc::clone(&active_readers);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                rwlock.read_lock(rid + 1);
                active_readers.fetch_add(1, Ordering::Relaxed);
                let _ = shared.load(Ordering::Relaxed);
                thread::yield_now();
                active_readers.fetch_sub(1, Ordering::Relaxed);
                rwlock.read_unlock();
            }
        }));
    }

    for wid in 0..n_writers {
        let rwlock = Arc::clone(&rwlock);
        let shared = Arc::clone(&shared);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                rwlock.write_lock(n_readers + wid + 1);
                shared.fetch_add(1, Ordering::Relaxed);
                thread::yield_now();
                rwlock.write_unlock();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        shared.load(Ordering::Relaxed),
        n_writers * 50,
        "Writer count mismatch"
    );
}

// ===========================================================================
// 反向测试（故意引入错误 -> 检测问题）
// ===========================================================================

#[test]
fn test_negative_missing_unlock_timeout() {
    // 如果一个线程获取锁但不释放，另一个线程应该无法获取
    let mutex = Arc::new(MutexBlocking::new());

    let m = Arc::clone(&mutex);
    let holder = thread::spawn(move || {
        m.acquire(1);
        // 故意不 release，持锁 200ms
        thread::sleep(Duration::from_millis(200));
        m.release();
    });

    // 主线程尝试在 50ms 内获取，应该失败
    thread::sleep(Duration::from_millis(10));
    let start = Instant::now();
    let acquired = mutex.lock(2);
    let elapsed = start.elapsed();

    assert!(
        !acquired,
        "Should not acquire lock while holder hasn't released"
    );
    assert!(
        elapsed < Duration::from_millis(50),
        "lock() (non-blocking) should return immediately"
    );

    holder.join().unwrap();
}

#[test]
fn test_negative_semaphore_over_down() {
    let sem = Semaphore::new(1);
    assert!(sem.down(1));
    assert!(!sem.down(2));
    assert_eq!(sem.count(), -1);
    assert_eq!(sem.waiting_count(), 1);
}

#[test]
fn test_negative_dining_philosophers_potential_deadlock() {
    // 无序获取叉子 + 很短超时 -> 观测潜在死锁
    let result = dining_philosophers(5, 100, Duration::from_secs(2), false);
    // 不断言 deadlock_detected == true（非确定性），但输出结果
    println!(
        "Unordered dining: meals={}/{}, deadlock={}",
        result.total_meals,
        5 * 100,
        result.deadlock_detected,
    );
}

#[test]
fn test_negative_dining_philosophers_ordered_no_deadlock() {
    let result = dining_philosophers(5, 100, Duration::from_secs(5), true);
    assert!(
        !result.deadlock_detected,
        "Ordered fork acquisition should never deadlock"
    );
    assert_eq!(
        result.total_meals,
        5 * 100,
        "All philosophers should finish all meals"
    );
}

// ===========================================================================
// Instrumented 测试
// ===========================================================================

#[test]
fn test_instrumented_mutex_metrics() {
    let im = Arc::new(InstrumentedMutex::new(MutexBlocking::new()));
    let n_threads = 4;
    let n_iterations = 100;

    let handles: Vec<_> = (0..n_threads)
        .map(|tid| {
            let im = Arc::clone(&im);
            thread::spawn(move || {
                for _ in 0..n_iterations {
                    im.blocking_lock(tid + 1);
                    std::hint::spin_loop();
                    im.blocking_unlock(tid + 1);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let m = im.metrics();
    println!("Instrumented Mutex metrics:\n{m}");
    assert_eq!(
        m.acquire_count,
        (n_threads * n_iterations) as u64,
        "All acquires should be counted"
    );
    assert!(
        m.per_thread_acquire_count.len() == n_threads,
        "Each thread should have acquire counts"
    );
}

// ===========================================================================
// 对比测试
// ===========================================================================

#[test]
fn test_comparison_spin_vs_sleep() {
    let n_threads = 4;
    let n_iterations = 200;
    let counter_spin = Arc::new(AtomicUsize::new(0));
    let counter_sleep = Arc::new(AtomicUsize::new(0));

    // Spin lock
    let spin = Arc::new(SpinLock::new());
    let spin_start = Instant::now();
    let handles: Vec<_> = (0..n_threads)
        .map(|tid| {
            let lock = Arc::clone(&spin);
            let counter = Arc::clone(&counter_spin);
            thread::spawn(move || {
                for _ in 0..n_iterations {
                    lock.acquire(tid + 1);
                    counter.fetch_add(1, Ordering::Relaxed);
                    lock.release();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let spin_elapsed = spin_start.elapsed();

    // Sleep lock
    let sleep = Arc::new(MutexBlocking::new());
    let sleep_start = Instant::now();
    let handles: Vec<_> = (0..n_threads)
        .map(|tid| {
            let lock = Arc::clone(&sleep);
            let counter = Arc::clone(&counter_sleep);
            thread::spawn(move || {
                for _ in 0..n_iterations {
                    lock.acquire(tid + 1);
                    counter.fetch_add(1, Ordering::Relaxed);
                    lock.release();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let sleep_elapsed = sleep_start.elapsed();

    println!("Spin  lock: {}us for {} ops", spin_elapsed.as_micros(), n_threads * n_iterations);
    println!("Sleep lock: {}us for {} ops", sleep_elapsed.as_micros(), n_threads * n_iterations);
    println!("Ratio (sleep/spin): {:.2}x", sleep_elapsed.as_micros() as f64 / spin_elapsed.as_micros().max(1) as f64);

    assert_eq!(counter_spin.load(Ordering::Relaxed), n_threads * n_iterations);
    assert_eq!(counter_sleep.load(Ordering::Relaxed), n_threads * n_iterations);
}

// ===========================================================================
// 场景对比总表
// ===========================================================================

#[test]
fn test_scenario_producer_consumer() {
    let result = producer_consumer(8, 4, 4, 200);
    println!("Producer-Consumer:");
    println!("  produced={}, consumed={}", result.produced, result.consumed);
    println!("{}", result.metrics);
    assert_eq!(result.produced, 4 * 200);
    assert_eq!(result.consumed, result.produced);
}

#[test]
fn test_scenario_readers_writers() {
    let rw = readers_writers(6, 2, 100, 5);
    println!("Readers-Writers (RwLock):");
    println!("  reads={}, writes={}", rw.total_reads, rw.total_writes);
    println!("{}", rw.metrics);

    let mx = readers_writers_mutex(6, 2, 100, 5);
    println!("Readers-Writers (Mutex):");
    println!("  reads={}, writes={}", mx.total_reads, mx.total_writes);
    println!("{}", mx.metrics);
}

#[test]
fn test_full_comparison() {
    let output = run_all_comparisons();
    println!("{output}");
}

#[test]
fn test_quick_comparison() {
    let output = run_quick_comparison();
    println!("{output}");
}
