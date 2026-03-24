//! 同步原语集成测试
//!
//! 包含：
//! - 正向功能测试（每个原语的基本正确性）
//! - 反向测试（故意缺少 unlock / 错误顺序 -> 检测死锁/超时）
//! - 多线程压力测试
//! - 对比测试（spin vs sleep / rwlock vs mutex）

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
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
//
// 设计原则：每个原语各一个"故意出错"的对照测试。
// 测试**本身会通过**，但通过超时机制检测并报告了错误现象。
// 用 `--nocapture` 运行可看到诊断输出。
// ===========================================================================

/// 辅助：在子线程中执行 `f`，等待 `timeout` 后判定是否卡住。
/// 返回 `true` 表示子线程在超时前完成，`false` 表示超时（疑似死锁/活锁）。
fn completes_within<F: FnOnce() + Send + 'static>(timeout: Duration, f: F) -> bool {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(timeout).is_ok()
}

// ── 1. SpinLock：故意不 release → 第二个线程自旋卡死 ──

#[test]
fn test_negative_spinlock_missing_release() {
    let lock = Arc::new(SpinLock::new());

    // 线程 A 获取锁后「忘记」release
    lock.acquire(1);

    // 线程 B 尝试 acquire，应该永远拿不到
    let lock2 = Arc::clone(&lock);
    let finished = completes_within(Duration::from_millis(500), move || {
        lock2.acquire(2);
    });

    println!(
        "[NEG-SpinLock] missing release → second acquire finished={} (expected false)",
        finished
    );
    assert!(
        !finished,
        "SpinLock: acquire() should hang when previous holder never releases"
    );

    // 清理：释放锁让后台线程终止
    lock.release();
    thread::sleep(Duration::from_millis(50));
}

// ── 2. MutexBlocking：故意不 release → 第二个线程阻塞超时 ──

#[test]
fn test_negative_mutex_blocking_missing_release() {
    let mutex = Arc::new(MutexBlocking::new());
    mutex.acquire(1); // 主线程持锁

    // 线程 B 尝试 acquire，预期超时
    let m2 = Arc::clone(&mutex);
    let finished = completes_within(Duration::from_millis(500), move || {
        m2.acquire(2);
    });

    println!(
        "[NEG-MutexBlocking] missing release → second acquire finished={} (expected false)",
        finished
    );
    assert!(
        !finished,
        "MutexBlocking: acquire() should hang when previous holder never releases"
    );

    // try_acquire_timeout 也应该返回 false
    let m3 = Arc::clone(&mutex);
    let got_it = m3.try_acquire_timeout(3, Duration::from_millis(200));
    println!(
        "[NEG-MutexBlocking] try_acquire_timeout → got_it={} (expected false)",
        got_it
    );
    assert!(!got_it, "try_acquire_timeout should fail while lock is held");

    mutex.release();
    thread::sleep(Duration::from_millis(50));
}

// ── 3. Semaphore：故意不 up/release → acquire 永远阻塞 ──

#[test]
fn test_negative_semaphore_missing_release() {
    let sem = Arc::new(Semaphore::new(1));

    // 消耗唯一的资源
    sem.acquire(1);

    // 第二个 acquire 应该阻塞（没有人 release）
    let s2 = Arc::clone(&sem);
    let finished = completes_within(Duration::from_millis(500), move || {
        s2.acquire(2);
    });

    println!(
        "[NEG-Semaphore] missing release → second acquire finished={} (expected false)",
        finished
    );
    assert!(
        !finished,
        "Semaphore: acquire() should hang when no one calls release/up"
    );

    // 验证 down() 非阻塞接口也正确返回 false
    let blocked = !sem.down(3);
    println!(
        "[NEG-Semaphore] down() after exhaustion → blocked={} (expected true), count={}",
        blocked,
        sem.count()
    );
    assert!(blocked, "down() should return false when count < 0");

    sem.release(1);
    thread::sleep(Duration::from_millis(50));
}

// ── 4. Condvar：故意不 signal → wait 永远阻塞 ──

#[test]
fn test_negative_condvar_missing_signal() {
    let cv = Arc::new(Condvar::new());
    let mutex = Arc::new(MutexBlocking::new());
    let woke_up = Arc::new(AtomicBool::new(false));

    // 通过 wait_no_sched 注册等待者（模拟内核调度层）
    cv.wait_no_sched(1);
    assert_eq!(cv.waiting_count(), 1);

    // 用 wait_with_mutex 模拟完整的 condvar wait
    let cv2 = Arc::clone(&cv);
    let m2: Arc<dyn Mutex> = Arc::clone(&mutex) as Arc<dyn Mutex>;
    let woke2 = Arc::clone(&woke_up);
    let finished = completes_within(Duration::from_millis(500), move || {
        m2.lock(10);
        let _ = cv2.wait_with_mutex(10, m2);
        woke2.store(true, Ordering::Relaxed);
    });

    println!(
        "[NEG-Condvar] missing signal → wait returned={}, woke_up={} \
         (wait_with_mutex without signal still returns because it's a non-blocking kernel API)",
        finished,
        woke_up.load(Ordering::Relaxed)
    );

    // signal()==None 时无人被唤醒
    let cv3 = Condvar::new();
    let nobody = cv3.signal();
    println!(
        "[NEG-Condvar] signal on empty queue → {:?} (expected None)",
        nobody
    );
    assert_eq!(nobody, None, "signal() on empty queue should return None");

    // 注册等待者但不 signal → waiting_count 保持非零
    cv3.wait_no_sched(1);
    cv3.wait_no_sched(2);
    println!(
        "[NEG-Condvar] 2 waiters registered, no signal → waiting_count={} (expected 2)",
        cv3.waiting_count()
    );
    assert_eq!(
        cv3.waiting_count(),
        2,
        "Waiters should remain queued without signal"
    );
}

// ── 5. RwLock：故意不 write_unlock → 读者和写者全部卡死 ──

#[test]
fn test_negative_rwlock_missing_write_unlock() {
    let rw = Arc::new(RwLock::new());

    // 写者拿锁后「忘记」unlock
    rw.write_lock(1);

    // 读者尝试获取读锁 → 应该卡住
    let rw2 = Arc::clone(&rw);
    let reader_ok = completes_within(Duration::from_millis(500), move || {
        rw2.read_lock(2);
    });

    println!(
        "[NEG-RwLock] missing write_unlock → read_lock finished={} (expected false)",
        reader_ok
    );
    assert!(
        !reader_ok,
        "RwLock: read_lock() should hang when writer never unlocks"
    );

    // 另一个写者也应该卡住
    let rw3 = Arc::clone(&rw);
    let writer_ok = completes_within(Duration::from_millis(500), move || {
        rw3.write_lock(3);
    });

    println!(
        "[NEG-RwLock] missing write_unlock → write_lock finished={} (expected false)",
        writer_ok
    );
    assert!(
        !writer_ok,
        "RwLock: write_lock() should hang when writer never unlocks"
    );

    rw.write_unlock();
    thread::sleep(Duration::from_millis(50));
}

// ── 6. 哲学家就餐：无序获取 → 观测死锁 vs 有序获取 → 无死锁 ──

#[test]
fn test_negative_dining_philosophers_potential_deadlock() {
    let result = dining_philosophers(5, 100, Duration::from_secs(2), false);
    println!(
        "[NEG-DiningPhil] Unordered: meals={}/{}, deadlock={}",
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
