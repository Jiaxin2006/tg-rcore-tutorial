//! 同步原语实现
//!
//! 提供 5 种同步原语，接口与 `tg-rcore-tutorial-sync` 的内核 trait 保持一致：
//!
//! - [`SpinLock`]：基于 `AtomicBool` CAS 的自旋锁
//! - [`MutexBlocking`]：基于 `std::sync` 的睡眠锁
//! - [`Semaphore`]：经典计数信号量
//! - [`Condvar`]：条件变量
//! - [`RwLock`]：读写锁（原框架没有）

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{self, Arc};

/// 线程 ID 类型，与内核 `tg_task_manage::ThreadId` 对应。
pub type ThreadId = usize;

// ---------------------------------------------------------------------------
// Mutex trait — 与内核 tg-rcore-tutorial-sync/src/mutex.rs 完全一致
// ---------------------------------------------------------------------------

/// 互斥锁 trait，匹配内核 `tg_sync::Mutex` 接口。
///
/// 返回值语义：
/// - `lock(tid) -> true`：成功获取锁
/// - `lock(tid) -> false`：锁被占用，调用方应阻塞该线程
/// - `unlock() -> Some(tid)`：释放锁并唤醒等待线程 `tid`
/// - `unlock() -> None`：释放锁，无等待者
pub trait Mutex: Send + Sync {
    /// 尝试获取锁。
    fn lock(&self, tid: ThreadId) -> bool;
    /// 释放锁。
    fn unlock(&self) -> Option<ThreadId>;
    /// 当前持锁者。
    fn holder(&self) -> Option<ThreadId>;
    /// 等待队列快照。
    fn waiting(&self) -> Vec<ThreadId>;
}

// ===========================================================================
// 1. SpinLock
// ===========================================================================

/// 基于 `AtomicBool` CAS 的自旋锁。
///
/// 在用户态测试中使用 `std::hint::spin_loop` 降低 CPU 消耗；
/// 若嵌入内核则对应"关中断 + 原子自旋"场景。
pub struct SpinLock {
    locked: AtomicBool,
    holder: std::sync::Mutex<Option<ThreadId>>,
    wait_queue: std::sync::Mutex<VecDeque<ThreadId>>,
}

impl SpinLock {
    /// 创建一个新的自旋锁。
    pub fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            holder: std::sync::Mutex::new(None),
            wait_queue: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// 自旋获取锁（阻塞式，实际自旋等待直到成功）。
    pub fn acquire(&self, tid: ThreadId) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        *self.holder.lock().unwrap() = Some(tid);
    }

    /// 释放自旋锁。
    pub fn release(&self) -> Option<ThreadId> {
        *self.holder.lock().unwrap() = None;
        self.locked.store(false, Ordering::Release);
        None
    }
}

impl Mutex for SpinLock {
    fn lock(&self, tid: ThreadId) -> bool {
        match self.locked.compare_exchange(
            false,
            true,
            Ordering::Acquire,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                *self.holder.lock().unwrap() = Some(tid);
                true
            }
            Err(_) => {
                self.wait_queue.lock().unwrap().push_back(tid);
                false
            }
        }
    }

    fn unlock(&self) -> Option<ThreadId> {
        let mut wq = self.wait_queue.lock().unwrap();
        if let Some(waking) = wq.pop_front() {
            *self.holder.lock().unwrap() = Some(waking);
            Some(waking)
        } else {
            *self.holder.lock().unwrap() = None;
            self.locked.store(false, Ordering::Release);
            None
        }
    }

    fn holder(&self) -> Option<ThreadId> {
        *self.holder.lock().unwrap()
    }

    fn waiting(&self) -> Vec<ThreadId> {
        self.wait_queue.lock().unwrap().iter().copied().collect()
    }
}

// ===========================================================================
// 2. MutexBlocking — 睡眠锁
// ===========================================================================

/// 基于 `std::sync::Mutex` + `std::sync::Condvar` 的睡眠互斥锁。
///
/// 接口匹配内核 `tg_sync::MutexBlocking`。
pub struct MutexBlocking {
    inner: std::sync::Mutex<MutexBlockingInner>,
    condvar: sync::Condvar,
}

struct MutexBlockingInner {
    locked: bool,
    holder: Option<ThreadId>,
    wait_queue: VecDeque<ThreadId>,
}

impl MutexBlocking {
    /// 创建一个新的阻塞互斥锁。
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(MutexBlockingInner {
                locked: false,
                holder: None,
                wait_queue: VecDeque::new(),
            }),
            condvar: sync::Condvar::new(),
        }
    }

    /// 阻塞式获取锁（实际 sleep 直到获取成功）。
    pub fn acquire(&self, tid: ThreadId) {
        let mut inner = self.inner.lock().unwrap();
        while inner.locked {
            inner.wait_queue.push_back(tid);
            inner = self.condvar.wait(inner).unwrap();
            if let Some(pos) = inner.wait_queue.iter().position(|&t| t == tid) {
                inner.wait_queue.remove(pos);
            }
        }
        inner.locked = true;
        inner.holder = Some(tid);
    }

    /// 带超时的阻塞式获取，返回是否成功。
    pub fn try_acquire_timeout(&self, tid: ThreadId, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut inner = self.inner.lock().unwrap();
        while inner.locked {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                if let Some(pos) = inner.wait_queue.iter().position(|&t| t == tid) {
                    inner.wait_queue.remove(pos);
                }
                return false;
            }
            inner.wait_queue.push_back(tid);
            let (guard, _timeout_result) = self.condvar.wait_timeout(inner, remaining).unwrap();
            inner = guard;
            if let Some(pos) = inner.wait_queue.iter().position(|&t| t == tid) {
                inner.wait_queue.remove(pos);
            }
        }
        inner.locked = true;
        inner.holder = Some(tid);
        true
    }

    /// 释放锁并唤醒一个等待者。
    pub fn release(&self) -> Option<ThreadId> {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.locked);
        if let Some(waking) = inner.wait_queue.pop_front() {
            inner.holder = Some(waking);
            self.condvar.notify_all();
            Some(waking)
        } else {
            inner.locked = false;
            inner.holder = None;
            None
        }
    }
}

impl Mutex for MutexBlocking {
    fn lock(&self, tid: ThreadId) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.locked {
            inner.wait_queue.push_back(tid);
            false
        } else {
            inner.locked = true;
            inner.holder = Some(tid);
            true
        }
    }

    fn unlock(&self) -> Option<ThreadId> {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.locked);
        if let Some(waking) = inner.wait_queue.pop_front() {
            inner.holder = Some(waking);
            Some(waking)
        } else {
            inner.locked = false;
            inner.holder = None;
            None
        }
    }

    fn holder(&self) -> Option<ThreadId> {
        self.inner.lock().unwrap().holder
    }

    fn waiting(&self) -> Vec<ThreadId> {
        self.inner.lock().unwrap().wait_queue.iter().copied().collect()
    }
}

// ===========================================================================
// 3. Semaphore
// ===========================================================================

/// 经典计数信号量，接口匹配内核 `tg_sync::Semaphore`。
///
/// 语义：
/// - `count >= 0`：可用资源数
/// - `count < 0`：有 `-count` 个线程在等待
pub struct Semaphore {
    inner: std::sync::Mutex<SemaphoreInner>,
    condvar: sync::Condvar,
}

struct SemaphoreInner {
    count: isize,
    wait_queue: VecDeque<ThreadId>,
}

impl Semaphore {
    /// 创建信号量，初始资源数 = `res_count`。
    pub fn new(res_count: usize) -> Self {
        Self {
            inner: std::sync::Mutex::new(SemaphoreInner {
                count: res_count as isize,
                wait_queue: VecDeque::new(),
            }),
            condvar: sync::Condvar::new(),
        }
    }

    /// V 操作（释放）：计数 +1，若有等待者则唤醒。
    pub fn up(&self, _tid: ThreadId) -> Option<ThreadId> {
        let mut inner = self.inner.lock().unwrap();
        inner.count += 1;
        if let Some(waking) = inner.wait_queue.pop_front() {
            self.condvar.notify_all();
            Some(waking)
        } else {
            None
        }
    }

    /// P 操作（获取）：计数 -1，不足则阻塞。
    ///
    /// 内核接口语义：返回 `false` 表示需要阻塞。
    pub fn down(&self, tid: ThreadId) -> bool {
        let mut inner = self.inner.lock().unwrap();
        inner.count -= 1;
        if inner.count < 0 {
            inner.wait_queue.push_back(tid);
            false
        } else {
            true
        }
    }

    /// 阻塞式 P 操作（实际等待直到获取资源）。
    pub fn acquire(&self, tid: ThreadId) {
        let mut inner = self.inner.lock().unwrap();
        inner.count -= 1;
        if inner.count < 0 {
            inner.wait_queue.push_back(tid);
            while inner.wait_queue.contains(&tid) {
                inner = self.condvar.wait(inner).unwrap();
            }
        }
    }

    /// V 操作的阻塞版（唤醒一个等待者）。
    pub fn release(&self, tid: ThreadId) {
        let mut inner = self.inner.lock().unwrap();
        inner.count += 1;
        if let Some(_waking) = inner.wait_queue.pop_front() {
            drop(inner);
            self.condvar.notify_all();
        }
        let _ = tid;
    }

    /// 当前计数。
    pub fn count(&self) -> isize {
        self.inner.lock().unwrap().count
    }

    /// 等待队列长度。
    pub fn waiting_count(&self) -> usize {
        self.inner.lock().unwrap().wait_queue.len()
    }
}

// ===========================================================================
// 4. Condvar
// ===========================================================================

/// 条件变量，接口匹配内核 `tg_sync::Condvar`。
pub struct Condvar {
    inner: std::sync::Mutex<CondvarInner>,
    sys_condvar: sync::Condvar,
}

struct CondvarInner {
    wait_queue: VecDeque<ThreadId>,
}

impl Condvar {
    /// 创建新的条件变量。
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(CondvarInner {
                wait_queue: VecDeque::new(),
            }),
            sys_condvar: sync::Condvar::new(),
        }
    }

    /// 唤醒一个等待线程（内核接口）。
    pub fn signal(&self) -> Option<ThreadId> {
        let mut inner = self.inner.lock().unwrap();
        let result = inner.wait_queue.pop_front();
        if result.is_some() {
            self.sys_condvar.notify_one();
        }
        result
    }

    /// 将线程标记为等待（内核接口，不调度）。
    pub fn wait_no_sched(&self, tid: ThreadId) -> bool {
        self.inner.lock().unwrap().wait_queue.push_back(tid);
        false
    }

    /// 释放 mutex + 等待条件变量 + 重新获取 mutex（内核接口）。
    ///
    /// 简化实现，与内核 `tg_sync::Condvar::wait_with_mutex` 一致。
    pub fn wait_with_mutex(
        &self,
        tid: ThreadId,
        mutex: Arc<dyn Mutex>,
    ) -> (bool, Option<ThreadId>) {
        let waking_tid = mutex.unlock();
        (mutex.lock(tid), waking_tid)
    }

    /// 等待队列长度。
    pub fn waiting_count(&self) -> usize {
        self.inner.lock().unwrap().wait_queue.len()
    }
}

// ===========================================================================
// 5. RwLock — 读写锁（原框架不含）
// ===========================================================================

/// 读写锁：支持多读者并发 / 单写者独占。
///
/// 原 `tg-rcore-tutorial-sync` 框架不含此原语；
/// 若嵌入内核需在 `SyncMutex` trait 添加 rwlock 相关系统调用。
pub struct RwLock {
    inner: std::sync::Mutex<RwLockInner>,
    read_cv: sync::Condvar,
    write_cv: sync::Condvar,
}

struct RwLockInner {
    reader_count: usize,
    writer_active: bool,
    writer_holder: Option<ThreadId>,
    waiting_writers: VecDeque<ThreadId>,
    waiting_readers: VecDeque<ThreadId>,
}

impl RwLock {
    /// 创建新的读写锁。
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(RwLockInner {
                reader_count: 0,
                writer_active: false,
                writer_holder: None,
                waiting_writers: VecDeque::new(),
                waiting_readers: VecDeque::new(),
            }),
            read_cv: sync::Condvar::new(),
            write_cv: sync::Condvar::new(),
        }
    }

    /// 获取读锁（阻塞式）。
    pub fn read_lock(&self, tid: ThreadId) {
        let mut inner = self.inner.lock().unwrap();
        while inner.writer_active || !inner.waiting_writers.is_empty() {
            inner.waiting_readers.push_back(tid);
            inner = self.read_cv.wait(inner).unwrap();
            if let Some(pos) = inner.waiting_readers.iter().position(|&t| t == tid) {
                inner.waiting_readers.remove(pos);
            }
        }
        inner.reader_count += 1;
    }

    /// 释放读锁。
    pub fn read_unlock(&self) {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.reader_count > 0);
        inner.reader_count -= 1;
        if inner.reader_count == 0 {
            self.write_cv.notify_one();
        }
    }

    /// 获取写锁（阻塞式）。
    pub fn write_lock(&self, tid: ThreadId) {
        let mut inner = self.inner.lock().unwrap();
        while inner.writer_active || inner.reader_count > 0 {
            inner.waiting_writers.push_back(tid);
            inner = self.write_cv.wait(inner).unwrap();
            if let Some(pos) = inner.waiting_writers.iter().position(|&t| t == tid) {
                inner.waiting_writers.remove(pos);
            }
        }
        inner.writer_active = true;
        inner.writer_holder = Some(tid);
    }

    /// 释放写锁。
    pub fn write_unlock(&self) {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.writer_active);
        inner.writer_active = false;
        inner.writer_holder = None;
        if !inner.waiting_writers.is_empty() {
            self.write_cv.notify_one();
        } else {
            self.read_cv.notify_all();
        }
    }

    /// 当前活跃读者数。
    pub fn reader_count(&self) -> usize {
        self.inner.lock().unwrap().reader_count
    }

    /// 是否有写者活跃。
    pub fn writer_active(&self) -> bool {
        self.inner.lock().unwrap().writer_active
    }

    /// 等待中的写者数。
    pub fn waiting_writers(&self) -> usize {
        self.inner.lock().unwrap().waiting_writers.len()
    }

    /// 等待中的读者数。
    pub fn waiting_readers(&self) -> usize {
        self.inner.lock().unwrap().waiting_readers.len()
    }
}

// ===========================================================================
// Thread-safe wrappers for concurrent testing
// ===========================================================================

/// 线程安全的阻塞式互斥锁包装，用于多线程场景测试。
///
/// 与 `Mutex` trait 的"返回 bool 让调用方决定是否阻塞"不同，
/// 此包装直接在内部完成阻塞等待，适合 `std::thread` 并发测试。
pub struct BlockingMutexGuard<'a> {
    lock: &'a MutexBlocking,
}

impl<'a> Drop for BlockingMutexGuard<'a> {
    fn drop(&mut self) {
        self.lock.release();
    }
}

impl MutexBlocking {
    /// RAII 风格获取锁。
    pub fn lock_guard(&self, tid: ThreadId) -> BlockingMutexGuard<'_> {
        self.acquire(tid);
        BlockingMutexGuard { lock: self }
    }
}
