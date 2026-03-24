//! 经典并发场景
//!
//! 每个场景返回 [`SyncMetrics`]，用于量化不同同步原语的行为差异。
//!
//! - [`producer_consumer`]：生产者-消费者（有界缓冲区）
//! - [`readers_writers`]：读者-写者
//! - [`dining_philosophers`]：哲学家就餐

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::metrics::SyncMetrics;
use crate::primitives::*;

// ===========================================================================
// 1. Producer–Consumer（有界缓冲区）
// ===========================================================================

/// 生产者-消费者场景的运行结果。
#[derive(Debug)]
pub struct ProducerConsumerResult {
    /// 汇总指标。
    pub metrics: SyncMetrics,
    /// 总耗时（微秒）。
    pub elapsed_us: u64,
    /// 实际生产的物品数。
    pub produced: u64,
    /// 实际消费的物品数。
    pub consumed: u64,
}

/// 使用 MutexBlocking + Semaphore 实现生产者-消费者。
///
/// - `buffer_size`：缓冲区大小
/// - `n_producers`：生产者线程数
/// - `n_consumers`：消费者线程数
/// - `items_per_producer`：每个生产者生产的物品数
pub fn producer_consumer(
    buffer_size: usize,
    n_producers: usize,
    n_consumers: usize,
    items_per_producer: usize,
) -> ProducerConsumerResult {
    let total_items = n_producers * items_per_producer;

    let mutex = Arc::new(MutexBlocking::new());
    let empty = Arc::new(Semaphore::new(buffer_size));
    let full = Arc::new(Semaphore::new(0));
    let buffer: Arc<std::sync::Mutex<Vec<usize>>> =
        Arc::new(std::sync::Mutex::new(Vec::with_capacity(buffer_size)));

    let produced = Arc::new(AtomicUsize::new(0));
    let consumed = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(std::sync::Mutex::new(SyncMetrics::default()));

    let start = Instant::now();

    let mut handles = Vec::new();

    for pid in 0..n_producers {
        let mutex = Arc::clone(&mutex);
        let empty = Arc::clone(&empty);
        let full = Arc::clone(&full);
        let buffer = Arc::clone(&buffer);
        let produced = Arc::clone(&produced);
        let metrics = Arc::clone(&metrics);
        let tid_base = pid + 1;

        handles.push(thread::spawn(move || {
            let mut local_metrics = SyncMetrics::default();
            for i in 0..items_per_producer {
                let wait_start = Instant::now();
                empty.acquire(tid_base);
                let wait_us = wait_start.elapsed().as_micros() as u64;
                if wait_us > 0 {
                    local_metrics.contention_count += 1;
                    local_metrics.total_wait_us += wait_us;
                    local_metrics.max_wait_us = local_metrics.max_wait_us.max(wait_us);
                    local_metrics.ctx_switch_count += 1;
                }

                let acq_start = Instant::now();
                mutex.acquire(tid_base);
                let acq_us = acq_start.elapsed().as_micros() as u64;
                if acq_us > 1 {
                    local_metrics.contention_count += 1;
                    local_metrics.total_wait_us += acq_us;
                    local_metrics.max_wait_us = local_metrics.max_wait_us.max(acq_us);
                }

                buffer.lock().unwrap().push(pid * items_per_producer + i);
                local_metrics.acquire_count += 1;
                let hold_start = Instant::now();
                produced.fetch_add(1, Ordering::Relaxed);

                let hold_us = hold_start.elapsed().as_micros() as u64;
                local_metrics.total_hold_us += hold_us;
                mutex.release();
                full.release(tid_base);
            }
            *local_metrics
                .per_thread_wait_us
                .entry(tid_base)
                .or_insert(0) += local_metrics.total_wait_us;
            *local_metrics
                .per_thread_acquire_count
                .entry(tid_base)
                .or_insert(0) += local_metrics.acquire_count;
            metrics.lock().unwrap().merge(&local_metrics);
        }));
    }

    for cid in 0..n_consumers {
        let mutex = Arc::clone(&mutex);
        let empty = Arc::clone(&empty);
        let full = Arc::clone(&full);
        let buffer = Arc::clone(&buffer);
        let consumed = Arc::clone(&consumed);
        let metrics = Arc::clone(&metrics);
        let tid_base = n_producers + cid + 1;

        handles.push(thread::spawn(move || {
            let mut local_metrics = SyncMetrics::default();
            loop {
                if consumed.load(Ordering::Relaxed) >= total_items {
                    break;
                }

                let wait_start = Instant::now();
                full.acquire(tid_base);
                let wait_us = wait_start.elapsed().as_micros() as u64;
                if wait_us > 0 {
                    local_metrics.contention_count += 1;
                    local_metrics.total_wait_us += wait_us;
                    local_metrics.max_wait_us = local_metrics.max_wait_us.max(wait_us);
                    local_metrics.ctx_switch_count += 1;
                }

                let acq_start = Instant::now();
                mutex.acquire(tid_base);
                let acq_us = acq_start.elapsed().as_micros() as u64;
                if acq_us > 1 {
                    local_metrics.contention_count += 1;
                    local_metrics.total_wait_us += acq_us;
                    local_metrics.max_wait_us = local_metrics.max_wait_us.max(acq_us);
                }

                let _ = buffer.lock().unwrap().pop();
                local_metrics.acquire_count += 1;
                consumed.fetch_add(1, Ordering::Relaxed);

                mutex.release();
                empty.release(tid_base);

                if consumed.load(Ordering::Relaxed) >= total_items {
                    break;
                }
            }
            *local_metrics
                .per_thread_wait_us
                .entry(tid_base)
                .or_insert(0) += local_metrics.total_wait_us;
            *local_metrics
                .per_thread_acquire_count
                .entry(tid_base)
                .or_insert(0) += local_metrics.acquire_count;
            metrics.lock().unwrap().merge(&local_metrics);
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    let elapsed_us = start.elapsed().as_micros() as u64;
    let final_metrics = metrics.lock().unwrap().clone();

    ProducerConsumerResult {
        metrics: final_metrics,
        elapsed_us,
        produced: produced.load(Ordering::Relaxed) as u64,
        consumed: consumed.load(Ordering::Relaxed) as u64,
    }
}

// ===========================================================================
// 2. Readers–Writers
// ===========================================================================

/// 读者-写者场景的运行结果。
#[derive(Debug)]
pub struct ReadersWritersResult {
    /// 汇总指标。
    pub metrics: SyncMetrics,
    /// 总耗时（微秒）。
    pub elapsed_us: u64,
    /// 总读取次数。
    pub total_reads: u64,
    /// 总写入次数。
    pub total_writes: u64,
}

/// 使用 `RwLock` 实现读者-写者场景。
///
/// - `n_readers`：读者线程数
/// - `n_writers`：写者线程数
/// - `iterations`：每个线程的操作次数
/// - `work_us`：模拟的临界区工作时间（微秒）
pub fn readers_writers(
    n_readers: usize,
    n_writers: usize,
    iterations: usize,
    work_us: u64,
) -> ReadersWritersResult {
    let rwlock = Arc::new(RwLock::new());
    let shared_data = Arc::new(AtomicUsize::new(0));
    let total_reads = Arc::new(AtomicUsize::new(0));
    let total_writes = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(std::sync::Mutex::new(SyncMetrics::default()));

    let start = Instant::now();
    let mut handles = Vec::new();

    for rid in 0..n_readers {
        let rwlock = Arc::clone(&rwlock);
        let shared_data = Arc::clone(&shared_data);
        let total_reads = Arc::clone(&total_reads);
        let metrics = Arc::clone(&metrics);
        let tid = rid + 1;

        handles.push(thread::spawn(move || {
            let mut local = SyncMetrics::default();
            for _ in 0..iterations {
                let wait_start = Instant::now();
                rwlock.read_lock(tid);
                let wait_us = wait_start.elapsed().as_micros() as u64;
                if wait_us > 1 {
                    local.contention_count += 1;
                    local.total_wait_us += wait_us;
                    local.max_wait_us = local.max_wait_us.max(wait_us);
                }
                local.acquire_count += 1;

                let _ = shared_data.load(Ordering::Relaxed);
                if work_us > 0 {
                    spin_work(work_us);
                }
                total_reads.fetch_add(1, Ordering::Relaxed);

                rwlock.read_unlock();
            }
            *local.per_thread_wait_us.entry(tid).or_insert(0) += local.total_wait_us;
            *local.per_thread_acquire_count.entry(tid).or_insert(0) += local.acquire_count;
            metrics.lock().unwrap().merge(&local);
        }));
    }

    for wid in 0..n_writers {
        let rwlock = Arc::clone(&rwlock);
        let shared_data = Arc::clone(&shared_data);
        let total_writes = Arc::clone(&total_writes);
        let metrics = Arc::clone(&metrics);
        let tid = n_readers + wid + 1;

        handles.push(thread::spawn(move || {
            let mut local = SyncMetrics::default();
            for _ in 0..iterations {
                let wait_start = Instant::now();
                rwlock.write_lock(tid);
                let wait_us = wait_start.elapsed().as_micros() as u64;
                if wait_us > 1 {
                    local.contention_count += 1;
                    local.total_wait_us += wait_us;
                    local.max_wait_us = local.max_wait_us.max(wait_us);
                    local.ctx_switch_count += 1;
                }
                local.acquire_count += 1;

                shared_data.fetch_add(1, Ordering::Relaxed);
                if work_us > 0 {
                    spin_work(work_us);
                }
                total_writes.fetch_add(1, Ordering::Relaxed);

                rwlock.write_unlock();
            }
            *local.per_thread_wait_us.entry(tid).or_insert(0) += local.total_wait_us;
            *local.per_thread_acquire_count.entry(tid).or_insert(0) += local.acquire_count;
            metrics.lock().unwrap().merge(&local);
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    let elapsed_us = start.elapsed().as_micros() as u64;

    ReadersWritersResult {
        metrics: metrics.lock().unwrap().clone(),
        elapsed_us,
        total_reads: total_reads.load(Ordering::Relaxed) as u64,
        total_writes: total_writes.load(Ordering::Relaxed) as u64,
    }
}

/// 使用 `MutexBlocking` 实现读者-写者（作为对照，所有操作串行化）。
pub fn readers_writers_mutex(
    n_readers: usize,
    n_writers: usize,
    iterations: usize,
    work_us: u64,
) -> ReadersWritersResult {
    let mutex = Arc::new(MutexBlocking::new());
    let shared_data = Arc::new(AtomicUsize::new(0));
    let total_reads = Arc::new(AtomicUsize::new(0));
    let total_writes = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(std::sync::Mutex::new(SyncMetrics::default()));

    let start = Instant::now();
    let mut handles = Vec::new();

    for rid in 0..n_readers {
        let mutex = Arc::clone(&mutex);
        let shared_data = Arc::clone(&shared_data);
        let total_reads = Arc::clone(&total_reads);
        let metrics = Arc::clone(&metrics);
        let tid = rid + 1;

        handles.push(thread::spawn(move || {
            let mut local = SyncMetrics::default();
            for _ in 0..iterations {
                let wait_start = Instant::now();
                mutex.acquire(tid);
                let wait_us = wait_start.elapsed().as_micros() as u64;
                if wait_us > 1 {
                    local.contention_count += 1;
                    local.total_wait_us += wait_us;
                    local.max_wait_us = local.max_wait_us.max(wait_us);
                    local.ctx_switch_count += 1;
                }
                local.acquire_count += 1;

                let _ = shared_data.load(Ordering::Relaxed);
                if work_us > 0 {
                    spin_work(work_us);
                }
                total_reads.fetch_add(1, Ordering::Relaxed);

                mutex.release();
            }
            *local.per_thread_wait_us.entry(tid).or_insert(0) += local.total_wait_us;
            *local.per_thread_acquire_count.entry(tid).or_insert(0) += local.acquire_count;
            metrics.lock().unwrap().merge(&local);
        }));
    }

    for wid in 0..n_writers {
        let mutex = Arc::clone(&mutex);
        let shared_data = Arc::clone(&shared_data);
        let total_writes = Arc::clone(&total_writes);
        let metrics = Arc::clone(&metrics);
        let tid = n_readers + wid + 1;

        handles.push(thread::spawn(move || {
            let mut local = SyncMetrics::default();
            for _ in 0..iterations {
                let wait_start = Instant::now();
                mutex.acquire(tid);
                let wait_us = wait_start.elapsed().as_micros() as u64;
                if wait_us > 1 {
                    local.contention_count += 1;
                    local.total_wait_us += wait_us;
                    local.max_wait_us = local.max_wait_us.max(wait_us);
                    local.ctx_switch_count += 1;
                }
                local.acquire_count += 1;

                shared_data.fetch_add(1, Ordering::Relaxed);
                if work_us > 0 {
                    spin_work(work_us);
                }
                total_writes.fetch_add(1, Ordering::Relaxed);

                mutex.release();
            }
            *local.per_thread_wait_us.entry(tid).or_insert(0) += local.total_wait_us;
            *local.per_thread_acquire_count.entry(tid).or_insert(0) += local.acquire_count;
            metrics.lock().unwrap().merge(&local);
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    let elapsed_us = start.elapsed().as_micros() as u64;

    ReadersWritersResult {
        metrics: metrics.lock().unwrap().clone(),
        elapsed_us,
        total_reads: total_reads.load(Ordering::Relaxed) as u64,
        total_writes: total_writes.load(Ordering::Relaxed) as u64,
    }
}

// ===========================================================================
// 3. Dining Philosophers
// ===========================================================================

/// 哲学家就餐场景的运行结果。
#[derive(Debug)]
pub struct DiningPhilosophersResult {
    /// 汇总指标。
    pub metrics: SyncMetrics,
    /// 总耗时（微秒）。
    pub elapsed_us: u64,
    /// 总进餐次数。
    pub total_meals: u64,
    /// 是否发生了死锁超时。
    pub deadlock_detected: bool,
}

/// 使用 MutexBlocking 实现哲学家就餐。
///
/// - `n_philosophers`：哲学家数量（= 叉子数量）
/// - `meals`：每个哲学家进餐次数
/// - `timeout`：超时时间（用于死锁检测）
/// - `use_ordered`：是否使用有序获取（避免死锁）
pub fn dining_philosophers(
    n_philosophers: usize,
    meals: usize,
    timeout: Duration,
    use_ordered: bool,
) -> DiningPhilosophersResult {
    let forks: Vec<Arc<MutexBlocking>> = (0..n_philosophers)
        .map(|_| Arc::new(MutexBlocking::new()))
        .collect();
    let total_meals = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(std::sync::Mutex::new(SyncMetrics::default()));
    let deadlock = Arc::new(AtomicBool::new(false));

    let start = Instant::now();
    let mut handles = Vec::new();

    for id in 0..n_philosophers {
        let left = Arc::clone(&forks[id]);
        let right = Arc::clone(&forks[(id + 1) % n_philosophers]);
        let total_meals = Arc::clone(&total_meals);
        let metrics = Arc::clone(&metrics);
        let deadlock = Arc::clone(&deadlock);
        let tid = id + 1;

        handles.push(thread::spawn(move || {
            let mut local = SyncMetrics::default();
            for _ in 0..meals {
                if deadlock.load(Ordering::Relaxed) {
                    break;
                }

                let (first, second) = if use_ordered {
                    // 有序获取：总是先拿编号小的叉子
                    if id < (id + 1) % (id + n_philosophers) {
                        (&left, &right)
                    } else {
                        (&right, &left)
                    }
                } else {
                    (&left, &right)
                };

                let wait_start = Instant::now();
                first.acquire(tid);
                let w1 = wait_start.elapsed().as_micros() as u64;

                // 短暂思考，增加死锁概率（无序模式下）
                if !use_ordered {
                    thread::yield_now();
                }

                let wait_start2 = Instant::now();
                second.acquire(tid);
                let w2 = wait_start2.elapsed().as_micros() as u64;

                let wait_total = w1 + w2;
                if wait_total > 1 {
                    local.contention_count += 1;
                    local.total_wait_us += wait_total;
                    local.max_wait_us = local.max_wait_us.max(wait_total);
                    local.ctx_switch_count += 1;
                }
                local.acquire_count += 1;

                // 进餐
                spin_work(10);
                total_meals.fetch_add(1, Ordering::Relaxed);

                second.release();
                first.release();
            }
            *local.per_thread_wait_us.entry(tid).or_insert(0) += local.total_wait_us;
            *local.per_thread_acquire_count.entry(tid).or_insert(0) += local.acquire_count;
            metrics.lock().unwrap().merge(&local);
        }));
    }

    // 超时检测
    let deadlock_flag = Arc::clone(&deadlock);
    let total_expected = n_philosophers * meals;
    let total_ref = Arc::clone(&total_meals);
    let watchdog = thread::spawn(move || {
        let deadline = Instant::now() + timeout;
        let mut last_count = 0;
        loop {
            thread::sleep(Duration::from_millis(50));
            let current = total_ref.load(Ordering::Relaxed);
            if current >= total_expected {
                return false;
            }
            if Instant::now() > deadline {
                deadlock_flag.store(true, Ordering::Relaxed);
                return true;
            }
            if current == last_count && Instant::now() > deadline - Duration::from_millis(100) {
                deadlock_flag.store(true, Ordering::Relaxed);
                return true;
            }
            last_count = current;
        }
    });

    for h in handles {
        let _ = h.join();
    }

    let deadlock_detected = watchdog.join().unwrap_or(false);
    let elapsed_us = start.elapsed().as_micros() as u64;

    DiningPhilosophersResult {
        metrics: metrics.lock().unwrap().clone(),
        elapsed_us,
        total_meals: total_meals.load(Ordering::Relaxed) as u64,
        deadlock_detected,
    }
}

// ===========================================================================
// Utility
// ===========================================================================

/// 模拟临界区工作的自旋等待。
fn spin_work(us: u64) {
    let start = Instant::now();
    while start.elapsed().as_micros() < us as u128 {
        std::hint::spin_loop();
    }
}

use std::sync::atomic::AtomicBool;
