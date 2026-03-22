use super::UPIntrFreeCell;
use alloc::collections::{BTreeMap, VecDeque};
use tg_task_manage::ThreadId;

// 教程说明：
// `count` 含义采用经典信号量语义：
// - count >= 0：可用资源数；
// - count < 0：有 `-count` 个线程在等待队列中。

/// Semaphore
pub struct Semaphore {
    /// UPIntrFreeCell<SemaphoreInner>
    pub inner: UPIntrFreeCell<SemaphoreInner>,
}

/// SemaphoreInner
pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<ThreadId>,
    pub allocation: BTreeMap<ThreadId, usize>,
}

impl Semaphore {
    /// 创建一个新的信号量，初始资源计数为 `res_count`。
    pub fn new(res_count: usize) -> Self {
        Self {
            // SAFETY: 此信号量仅在单处理器内核环境中使用
            inner: unsafe {
                UPIntrFreeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                    allocation: BTreeMap::new(),
                })
            },
        }
    }
    /// 当前线程释放信号量表示的一个资源，并唤醒一个阻塞的线程
    pub fn up(&self, tid: ThreadId) -> Option<ThreadId> {
        let mut inner = self.inner.exclusive_access();
        if let Some(v) = inner.allocation.get_mut(&tid) {
            if *v > 1 {
                *v -= 1;
            } else {
                inner.allocation.remove(&tid);
            }
        }
        inner.count += 1;
        // 若有等待者，交由调度器唤醒队首线程，并视作拿到一个资源。
        if let Some(waking_tid) = inner.wait_queue.pop_front() {
            *inner.allocation.entry(waking_tid).or_insert(0) += 1;
            Some(waking_tid)
        } else {
            None
        }
    }
    /// 当前线程试图获取信号量表示的资源，并返回结果
    pub fn down(&self, tid: ThreadId) -> bool {
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        if inner.count < 0 {
            // 资源不足：当前线程进入等待队列。
            inner.wait_queue.push_back(tid);
            drop(inner);
            false
        } else {
            *inner.allocation.entry(tid).or_insert(0) += 1;
            true
        }
    }

    /// 为死锁检测导出快照
    pub fn deadlock_snapshot(&self) -> (usize, VecDeque<ThreadId>, BTreeMap<ThreadId, usize>) {
        let inner = self.inner.exclusive_access();
        let available = if inner.count > 0 { inner.count as usize } else { 0 };
        (available, inner.wait_queue.clone(), inner.allocation.clone())
    }
}
