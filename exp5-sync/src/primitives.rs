//! 同步原语实现。
//!
//! `std` feature 下提供宿主机实验版本；
//! `kernel` feature 下提供 `no_std + alloc` 的内核可移植版本。

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(feature = "kernel")]
use crate::up::UPIntrFreeCell;
#[cfg(feature = "kernel")]
use alloc::collections::BTreeMap;

#[cfg(feature = "std")]
use std::sync::{self, Mutex as StdMutex};

/// 线程 ID 类型，与内核 `tg_task_manage::ThreadId` 对齐。
#[cfg(feature = "std")]
pub type ThreadId = usize;
/// 线程 ID 类型，与内核 `tg_task_manage::ThreadId` 对齐。
#[cfg(feature = "kernel")]
pub type ThreadId = tg_task_manage::ThreadId;

/// 互斥锁 trait，匹配内核 `tg_sync::Mutex` 接口。
pub trait Mutex: Send + Sync {
    fn lock(&self, tid: ThreadId) -> bool;
    fn unlock(&self) -> Option<ThreadId>;
    fn holder(&self) -> Option<ThreadId>;
    fn waiting(&self) -> Vec<ThreadId>;
    fn should_block_on_fail(&self) -> bool {
        true
    }
}

// ===========================================================================
// 1. SpinLock
// ===========================================================================

#[cfg(feature = "std")]
pub struct SpinLock {
    locked: AtomicBool,
    holder: StdMutex<Option<ThreadId>>,
    wait_queue: StdMutex<VecDeque<ThreadId>>,
}

#[cfg(feature = "kernel")]
pub struct SpinLock {
    locked: AtomicBool,
    inner: UPIntrFreeCell<SpinLockInner>,
}

#[cfg(feature = "kernel")]
struct SpinLockInner {
    holder: Option<ThreadId>,
    wait_queue: VecDeque<ThreadId>,
}

impl SpinLock {
    pub fn new() -> Self {
        #[cfg(feature = "std")]
        {
            Self {
                locked: AtomicBool::new(false),
                holder: StdMutex::new(None),
                wait_queue: StdMutex::new(VecDeque::new()),
            }
        }
        #[cfg(feature = "kernel")]
        {
            Self {
                locked: AtomicBool::new(false),
                inner: unsafe {
                    UPIntrFreeCell::new(SpinLockInner {
                        holder: None,
                        wait_queue: VecDeque::new(),
                    })
                },
            }
        }
    }

    #[cfg(feature = "std")]
    pub fn acquire(&self, tid: ThreadId) {
        let mut failures: u32 = 0;
        loop {
            if self.locked.load(Ordering::Relaxed) {
                Self::backoff(failures);
                failures = failures.saturating_add(1);
                continue;
            }
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                *self.holder.lock().unwrap() = Some(tid);
                return;
            }
            failures = failures.saturating_add(1);
        }
    }

    #[cfg(feature = "kernel")]
    pub fn acquire(&self, tid: ThreadId) {
        while !self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            core::hint::spin_loop();
        }
        self.inner.exclusive_session(|inner| {
            inner.holder = Some(tid);
        });
    }

    #[cfg(feature = "std")]
    fn backoff(failures: u32) {
        if failures < 4 {
            std::hint::spin_loop();
        } else if failures < 10 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(std::time::Duration::from_micros(1));
        }
    }

    pub fn release(&self) -> Option<ThreadId> {
        #[cfg(feature = "std")]
        {
            *self.holder.lock().unwrap() = None;
            self.locked.store(false, Ordering::Release);
            None
        }
        #[cfg(feature = "kernel")]
        {
            self.inner.exclusive_session(|inner| {
                inner.holder = None;
            });
            self.locked.store(false, Ordering::Release);
            None
        }
    }
}

#[cfg(feature = "std")]
impl Mutex for SpinLock {
    fn lock(&self, tid: ThreadId) -> bool {
        match self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
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

    fn should_block_on_fail(&self) -> bool {
        false
    }
}

#[cfg(feature = "kernel")]
impl Mutex for SpinLock {
    fn lock(&self, tid: ThreadId) -> bool {
        match self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => {
                self.inner.exclusive_session(|inner| {
                    inner.holder = Some(tid);
                });
                true
            }
            Err(_) => self.inner.exclusive_session(|inner| {
                inner.wait_queue.push_back(tid);
                false
            }),
        }
    }

    fn unlock(&self) -> Option<ThreadId> {
        self.inner.exclusive_session(|inner| {
            if let Some(waking) = inner.wait_queue.pop_front() {
                inner.holder = Some(waking);
                Some(waking)
            } else {
                inner.holder = None;
                self.locked.store(false, Ordering::Release);
                None
            }
        })
    }

    fn holder(&self) -> Option<ThreadId> {
        self.inner.exclusive_session(|inner| inner.holder)
    }

    fn waiting(&self) -> Vec<ThreadId> {
        self.inner
            .exclusive_session(|inner| inner.wait_queue.iter().copied().collect())
    }

    fn should_block_on_fail(&self) -> bool {
        false
    }
}

// ===========================================================================
// 2. MutexBlocking
// ===========================================================================

#[cfg(feature = "std")]
pub struct MutexBlocking {
    inner: StdMutex<MutexBlockingInner>,
    condvar: sync::Condvar,
}

#[cfg(feature = "kernel")]
pub struct MutexBlocking {
    inner: UPIntrFreeCell<MutexBlockingInner>,
}

pub struct MutexBlockingInner {
    locked: bool,
    holder: Option<ThreadId>,
    wait_queue: VecDeque<ThreadId>,
}

impl MutexBlocking {
    pub fn new() -> Self {
        #[cfg(feature = "std")]
        {
            Self {
                inner: StdMutex::new(MutexBlockingInner {
                    locked: false,
                    holder: None,
                    wait_queue: VecDeque::new(),
                }),
                condvar: sync::Condvar::new(),
            }
        }
        #[cfg(feature = "kernel")]
        {
            Self {
                inner: unsafe {
                    UPIntrFreeCell::new(MutexBlockingInner {
                        locked: false,
                        holder: None,
                        wait_queue: VecDeque::new(),
                    })
                },
            }
        }
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "std")]
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
            let (guard, _) = self.condvar.wait_timeout(inner, remaining).unwrap();
            inner = guard;
            if let Some(pos) = inner.wait_queue.iter().position(|&t| t == tid) {
                inner.wait_queue.remove(pos);
            }
        }
        inner.locked = true;
        inner.holder = Some(tid);
        true
    }

    #[cfg(feature = "std")]
    pub fn release(&self) -> Option<ThreadId> {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.locked);
        inner.locked = false;
        inner.holder = None;
        if let Some(waking) = inner.wait_queue.pop_front() {
            drop(inner);
            self.condvar.notify_all();
            Some(waking)
        } else {
            None
        }
    }
}

#[cfg(feature = "std")]
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

#[cfg(feature = "kernel")]
impl Mutex for MutexBlocking {
    fn lock(&self, tid: ThreadId) -> bool {
        self.inner.exclusive_session(|inner| {
            if inner.locked {
                inner.wait_queue.push_back(tid);
                false
            } else {
                inner.locked = true;
                inner.holder = Some(tid);
                true
            }
        })
    }

    fn unlock(&self) -> Option<ThreadId> {
        self.inner.exclusive_session(|inner| {
            assert!(inner.locked);
            if let Some(waking) = inner.wait_queue.pop_front() {
                inner.holder = Some(waking);
                Some(waking)
            } else {
                inner.locked = false;
                inner.holder = None;
                None
            }
        })
    }

    fn holder(&self) -> Option<ThreadId> {
        self.inner.exclusive_session(|inner| inner.holder)
    }

    fn waiting(&self) -> Vec<ThreadId> {
        self.inner
            .exclusive_session(|inner| inner.wait_queue.iter().copied().collect())
    }
}

// ===========================================================================
// 3. Semaphore
// ===========================================================================

#[cfg(feature = "std")]
pub struct Semaphore {
    inner: StdMutex<SemaphoreInner>,
    condvar: sync::Condvar,
}

#[cfg(feature = "kernel")]
pub struct Semaphore {
    pub inner: UPIntrFreeCell<SemaphoreInner>,
}

pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<ThreadId>,
    #[cfg(feature = "kernel")]
    pub allocation: BTreeMap<ThreadId, usize>,
}

impl Semaphore {
    pub fn new(res_count: usize) -> Self {
        #[cfg(feature = "std")]
        {
            Self {
                inner: StdMutex::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                }),
                condvar: sync::Condvar::new(),
            }
        }
        #[cfg(feature = "kernel")]
        {
            Self {
                inner: unsafe {
                    UPIntrFreeCell::new(SemaphoreInner {
                        count: res_count as isize,
                        wait_queue: VecDeque::new(),
                        allocation: BTreeMap::new(),
                    })
                },
            }
        }
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "kernel")]
    pub fn up(&self, tid: ThreadId) -> Option<ThreadId> {
        self.inner.exclusive_session(|inner| {
            if let Some(v) = inner.allocation.get_mut(&tid) {
                if *v > 1 {
                    *v -= 1;
                } else {
                    inner.allocation.remove(&tid);
                }
            }
            inner.count += 1;
            if let Some(waking_tid) = inner.wait_queue.pop_front() {
                *inner.allocation.entry(waking_tid).or_insert(0) += 1;
                Some(waking_tid)
            } else {
                None
            }
        })
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "kernel")]
    pub fn down(&self, tid: ThreadId) -> bool {
        self.inner.exclusive_session(|inner| {
            inner.count -= 1;
            if inner.count < 0 {
                inner.wait_queue.push_back(tid);
                false
            } else {
                *inner.allocation.entry(tid).or_insert(0) += 1;
                true
            }
        })
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "std")]
    pub fn release(&self, tid: ThreadId) {
        let mut inner = self.inner.lock().unwrap();
        inner.count += 1;
        if inner.wait_queue.pop_front().is_some() {
            drop(inner);
            self.condvar.notify_all();
        }
        let _ = tid;
    }

    #[cfg(feature = "std")]
    pub fn count(&self) -> isize {
        self.inner.lock().unwrap().count
    }

    #[cfg(feature = "std")]
    pub fn waiting_count(&self) -> usize {
        self.inner.lock().unwrap().wait_queue.len()
    }

    #[cfg(feature = "kernel")]
    pub fn deadlock_snapshot(&self) -> (usize, VecDeque<ThreadId>, BTreeMap<ThreadId, usize>) {
        self.inner.exclusive_session(|inner| {
            let available = if inner.count > 0 {
                inner.count as usize
            } else {
                0
            };
            (available, inner.wait_queue.clone(), inner.allocation.clone())
        })
    }
}

// ===========================================================================
// 4. Condvar
// ===========================================================================

#[cfg(feature = "std")]
pub struct Condvar {
    inner: StdMutex<CondvarInner>,
    sys_condvar: sync::Condvar,
}

#[cfg(feature = "kernel")]
pub struct Condvar {
    pub inner: UPIntrFreeCell<CondvarInner>,
}

pub struct CondvarInner {
    pub wait_queue: VecDeque<ThreadId>,
}

impl Condvar {
    pub fn new() -> Self {
        #[cfg(feature = "std")]
        {
            Self {
                inner: StdMutex::new(CondvarInner {
                    wait_queue: VecDeque::new(),
                }),
                sys_condvar: sync::Condvar::new(),
            }
        }
        #[cfg(feature = "kernel")]
        {
            Self {
                inner: unsafe {
                    UPIntrFreeCell::new(CondvarInner {
                        wait_queue: VecDeque::new(),
                    })
                },
            }
        }
    }

    #[cfg(feature = "std")]
    pub fn signal(&self) -> Option<ThreadId> {
        let mut inner = self.inner.lock().unwrap();
        let result = inner.wait_queue.pop_front();
        if result.is_some() {
            self.sys_condvar.notify_one();
        }
        result
    }

    #[cfg(feature = "kernel")]
    pub fn signal(&self) -> Option<ThreadId> {
        self.inner.exclusive_session(|inner| inner.wait_queue.pop_front())
    }

    #[cfg(feature = "std")]
    pub fn wait_no_sched(&self, tid: ThreadId) -> bool {
        self.inner.lock().unwrap().wait_queue.push_back(tid);
        false
    }

    #[cfg(feature = "kernel")]
    pub fn wait_no_sched(&self, tid: ThreadId) -> bool {
        self.inner.exclusive_session(|inner| {
            inner.wait_queue.push_back(tid);
        });
        false
    }

    pub fn wait_with_mutex(
        &self,
        tid: ThreadId,
        mutex: Arc<dyn Mutex>,
    ) -> (bool, Option<ThreadId>) {
        let waking_tid = mutex.unlock();
        (mutex.lock(tid), waking_tid)
    }

    #[cfg(feature = "std")]
    pub fn waiting_count(&self) -> usize {
        self.inner.lock().unwrap().wait_queue.len()
    }
}

// ===========================================================================
// 5. RwLock
// ===========================================================================

#[cfg(feature = "std")]
pub struct RwLock {
    inner: StdMutex<RwLockInner>,
    read_cv: sync::Condvar,
    write_cv: sync::Condvar,
}

#[cfg(feature = "kernel")]
pub struct RwLock {
    inner: UPIntrFreeCell<RwLockInner>,
}

struct RwLockInner {
    reader_count: usize,
    #[cfg(feature = "kernel")]
    reader_holders: BTreeMap<ThreadId, usize>,
    writer_active: bool,
    writer_holder: Option<ThreadId>,
    waiting_writers: VecDeque<ThreadId>,
    waiting_readers: VecDeque<ThreadId>,
}

impl RwLock {
    pub fn new() -> Self {
        #[cfg(feature = "std")]
        {
            Self {
                inner: StdMutex::new(RwLockInner {
                    reader_count: 0,
                    #[cfg(feature = "kernel")]
                    reader_holders: BTreeMap::new(),
                    writer_active: false,
                    writer_holder: None,
                    waiting_writers: VecDeque::new(),
                    waiting_readers: VecDeque::new(),
                }),
                read_cv: sync::Condvar::new(),
                write_cv: sync::Condvar::new(),
            }
        }
        #[cfg(feature = "kernel")]
        {
            Self {
                inner: unsafe {
                    UPIntrFreeCell::new(RwLockInner {
                        reader_count: 0,
                        reader_holders: BTreeMap::new(),
                        writer_active: false,
                        writer_holder: None,
                        waiting_writers: VecDeque::new(),
                        waiting_readers: VecDeque::new(),
                    })
                },
            }
        }
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "kernel")]
    pub fn read_lock(&self, tid: ThreadId) {
        loop {
            if self.inner.exclusive_session(|inner| {
                if inner.writer_active || !inner.waiting_writers.is_empty() {
                    if !inner.waiting_readers.contains(&tid) {
                        inner.waiting_readers.push_back(tid);
                    }
                    false
                } else {
                    if let Some(pos) = inner.waiting_readers.iter().position(|&t| t == tid) {
                        inner.waiting_readers.remove(pos);
                    }
                    inner.reader_count += 1;
                    *inner.reader_holders.entry(tid).or_insert(0) += 1;
                    true
                }
            }) {
                return;
            }
            core::hint::spin_loop();
        }
    }

    #[cfg(feature = "std")]
    pub fn read_unlock(&self) {
        let mut inner = self.inner.lock().unwrap();
        assert!(inner.reader_count > 0);
        inner.reader_count -= 1;
        if inner.reader_count == 0 {
            self.write_cv.notify_one();
        }
    }

    #[cfg(feature = "kernel")]
    pub fn read_unlock(&self) {
        self.inner.exclusive_session(|inner| {
            assert!(inner.reader_count > 0);
            inner.reader_count -= 1;
        });
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "kernel")]
    pub fn write_lock(&self, tid: ThreadId) {
        loop {
            if self.inner.exclusive_session(|inner| {
                if inner.writer_active || inner.reader_count > 0 {
                    if !inner.waiting_writers.contains(&tid) {
                        inner.waiting_writers.push_back(tid);
                    }
                    false
                } else {
                    if let Some(pos) = inner.waiting_writers.iter().position(|&t| t == tid) {
                        inner.waiting_writers.remove(pos);
                    }
                    inner.writer_active = true;
                    inner.writer_holder = Some(tid);
                    true
                }
            }) {
                return;
            }
            core::hint::spin_loop();
        }
    }

    #[cfg(feature = "std")]
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

    #[cfg(feature = "kernel")]
    pub fn write_unlock(&self) {
        self.inner.exclusive_session(|inner| {
            assert!(inner.writer_active);
            inner.writer_active = false;
            inner.writer_holder = None;
        });
    }

    #[cfg(feature = "std")]
    pub fn reader_count(&self) -> usize {
        self.inner.lock().unwrap().reader_count
    }

    #[cfg(feature = "kernel")]
    pub fn reader_count(&self) -> usize {
        self.inner.exclusive_session(|inner| inner.reader_count)
    }

    #[cfg(feature = "std")]
    pub fn writer_active(&self) -> bool {
        self.inner.lock().unwrap().writer_active
    }

    #[cfg(feature = "kernel")]
    pub fn writer_active(&self) -> bool {
        self.inner.exclusive_session(|inner| inner.writer_active)
    }

    #[cfg(feature = "std")]
    pub fn waiting_writers(&self) -> usize {
        self.inner.lock().unwrap().waiting_writers.len()
    }

    #[cfg(feature = "kernel")]
    pub fn waiting_writers(&self) -> usize {
        self.inner.exclusive_session(|inner| inner.waiting_writers.len())
    }

    #[cfg(feature = "std")]
    pub fn waiting_readers(&self) -> usize {
        self.inner.lock().unwrap().waiting_readers.len()
    }

    #[cfg(feature = "kernel")]
    pub fn waiting_readers(&self) -> usize {
        self.inner.exclusive_session(|inner| inner.waiting_readers.len())
    }

    #[cfg(feature = "kernel")]
    pub fn try_read(&self, tid: ThreadId) -> bool {
        self.inner.exclusive_session(|inner| {
            if inner.writer_active || !inner.waiting_writers.is_empty() {
                if !inner.waiting_readers.contains(&tid) {
                    inner.waiting_readers.push_back(tid);
                }
                false
            } else {
                if let Some(pos) = inner.waiting_readers.iter().position(|&t| t == tid) {
                    inner.waiting_readers.remove(pos);
                }
                inner.reader_count += 1;
                *inner.reader_holders.entry(tid).or_insert(0) += 1;
                true
            }
        })
    }

    #[cfg(feature = "kernel")]
    pub fn try_write(&self, tid: ThreadId) -> bool {
        self.inner.exclusive_session(|inner| {
            if inner.writer_active || inner.reader_count > 0 {
                if !inner.waiting_writers.contains(&tid) {
                    inner.waiting_writers.push_back(tid);
                }
                false
            } else {
                if let Some(pos) = inner.waiting_writers.iter().position(|&t| t == tid) {
                    inner.waiting_writers.remove(pos);
                }
                inner.writer_active = true;
                inner.writer_holder = Some(tid);
                true
            }
        })
    }

    #[cfg(feature = "kernel")]
    pub fn unlock_for(&self, tid: ThreadId) -> Vec<ThreadId> {
        self.inner.exclusive_session(|inner| {
            if inner.writer_holder == Some(tid) {
                inner.writer_active = false;
                inner.writer_holder = None;
            } else if let Some(count) = inner.reader_holders.get_mut(&tid) {
                assert!(inner.reader_count > 0);
                inner.reader_count -= 1;
                if *count > 1 {
                    *count -= 1;
                } else {
                    inner.reader_holders.remove(&tid);
                }
            } else {
                return Vec::new();
            }

            if !inner.writer_active && inner.reader_count == 0 {
                if let Some(waking_writer) = inner.waiting_writers.pop_front() {
                    inner.writer_active = true;
                    inner.writer_holder = Some(waking_writer);
                    let mut wakes = Vec::with_capacity(1);
                    wakes.push(waking_writer);
                    return wakes;
                }
                if !inner.waiting_readers.is_empty() {
                    let readers: Vec<_> = inner.waiting_readers.drain(..).collect();
                    inner.reader_count += readers.len();
                    for &reader in &readers {
                        *inner.reader_holders.entry(reader).or_insert(0) += 1;
                    }
                    return readers;
                }
            }
            Vec::new()
        })
    }
}

// ===========================================================================
// Thread-safe wrappers for std testing
// ===========================================================================

#[cfg(feature = "std")]
pub struct BlockingMutexGuard<'a> {
    lock: &'a MutexBlocking,
}

#[cfg(feature = "std")]
impl<'a> Drop for BlockingMutexGuard<'a> {
    fn drop(&mut self) {
        self.lock.release();
    }
}

#[cfg(feature = "std")]
impl MutexBlocking {
    pub fn lock_guard(&self, tid: ThreadId) -> BlockingMutexGuard<'_> {
        self.acquire(tid);
        BlockingMutexGuard { lock: self }
    }
}
