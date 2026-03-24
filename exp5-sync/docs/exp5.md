# 实验 5：同步互斥机制

> 从"能跑"到"公平/可证明不饿死"

## 学习目标

1. 理解自旋锁与睡眠锁的实现差异和适用场景
2. 掌握信号量、条件变量、读写锁的经典语义
3. 通过量化指标对比不同同步原语的性能特征
4. 通过反向测试验证正确性（去掉关键操作 → 测试必须失败）

## 背景知识

### 内核中的同步机制 (ch8)

rCore 教程 ch8 实现了以下同步原语：

| 原语 | 位置 | 接口 |
|------|------|------|
| `MutexBlocking` | `tg-rcore-tutorial-sync/src/mutex.rs` | `lock(tid)->bool`, `unlock()->Option<ThreadId>` |
| `Semaphore` | `tg-rcore-tutorial-sync/src/semaphore.rs` | `up(tid)->Option<ThreadId>`, `down(tid)->bool` |
| `Condvar` | `tg-rcore-tutorial-sync/src/condvar.rs` | `signal()->Option<ThreadId>`, `wait_with_mutex(...)` |
| `UPIntrFreeCell` | `tg-rcore-tutorial-sync/src/up.rs` | 关中断保护临界区 |

ch8 内核通过 `SyncMutex` syscall trait（`init_sync_mutex`）将这些原语暴露给用户态，
使用 `make_current_blocked()` / `re_enque(waking_tid)` 实现阻塞/唤醒。

### 本实验的扩展

| 扩展 | 说明 |
|------|------|
| `SpinLock` | 基于 `AtomicBool` CAS，原框架没有用户态 spinlock |
| `RwLock` | 读写锁，原框架没有此原语 |
| `InstrumentedMutex` | 指标采集装饰器，自动记录竞争/等待/持锁时间 |
| 经典场景 | 生产者-消费者、读者-写者、哲学家就餐 |
| 反向测试 | 故意缺少 unlock / 错误顺序 → 断言超时/死锁 |

## 文件结构

```
exp5-sync/
├── Cargo.toml
├── src/
│   ├── lib.rs          # 模块声明 + re-exports
│   ├── primitives.rs   # Mutex trait + 5 个同步原语
│   ├── metrics.rs      # SyncMetrics + InstrumentedMutex
│   ├── scenarios.rs    # 经典并发场景
│   └── workload.rs     # 对比引擎
├── tests/
│   └── sync_compare.rs # 集成测试
├── docs/
│   └── exp5.md         # 本文档
└── scripts/
    └── run_exp5.sh     # 一键运行
```

## API 说明

### Mutex trait（与内核一致）

```rust
pub trait Mutex: Send + Sync {
    fn lock(&self, tid: ThreadId) -> bool;
    fn unlock(&self) -> Option<ThreadId>;
    fn holder(&self) -> Option<ThreadId>;
    fn waiting(&self) -> Vec<ThreadId>;
}
```

- `lock` 返回 `false` 时，内核中对应 `make_current_blocked()`
- `unlock` 返回 `Some(tid)` 时，内核中对应 `re_enque(tid)`

### 各原语的阻塞式 API（用于多线程测试）

| 方法 | 说明 |
|------|------|
| `SpinLock::acquire(tid)` | CAS 自旋直到获取 |
| `MutexBlocking::acquire(tid)` | `Condvar::wait` 睡眠直到获取 |
| `Semaphore::acquire(tid)` / `release(tid)` | P/V 操作（阻塞式） |
| `RwLock::read_lock(tid)` / `write_lock(tid)` | 读/写锁获取 |

## 观测指标

| 指标 | 字段 | 说明 |
|------|------|------|
| 锁竞争次数 | `contention_count` | `lock()` 返回 `false` 的次数 |
| 平均持锁时间 | `avg_hold_us()` | 微秒 |
| 平均等待时间 | `avg_wait_us()` | 微秒 |
| 最大等待时间 | `max_wait_us` | 公平性指标 |
| 上下文切换 | `ctx_switch_count` | sleep lock yield 次数 |
| 饥饿事件 | `starvation_events` | 等待 > 10ms |
| 公平性 CV | `fairness_cv()` | 变异系数，越小越公平 |

## 运行方式

```bash
# 全部测试
cd exp5-sync
cargo test --test sync_compare -- --nocapture --test-threads=1

# 或使用脚本
./scripts/run_exp5.sh
```

## 嵌入内核的修改清单

若要将本实验的扩展嵌入 ch8 内核：

1. **SpinLock**：在 `tg-rcore-tutorial-sync/src/` 新增 `spinlock.rs`，
   使用 `UPIntrFreeCell` 替代 `std::sync::Mutex` 保护内部状态
2. **RwLock**：在 `tg-rcore-tutorial-sync/src/` 新增 `rwlock.rs`，
   需要在 `tg-rcore-tutorial-syscall` 的 `SyncMutex` trait 添加：
   - `rwlock_create`, `read_lock`, `write_lock`, `rwlock_unlock`
3. **Metrics**：在 ch8 的 `main.rs` 调度循环中，对 `MUTEX_LOCK` / `SEMAPHORE_DOWN` 等
   系统调用路径添加时间戳记录
4. **反向测试**：通过用户态测试程序（`tg-rcore-tutorial-user/src/bin/`）实现

## 讨论题

1. 自旋锁和睡眠锁在什么场景下各有优势？临界区长度如何影响选择？
2. 读写锁相比互斥锁在"多读少写"场景下提升了多少吞吐？代价是什么？
3. 哲学家就餐中，有序获取和无序获取的区别体现在哪些指标上？
4. `InstrumentedMutex` 装饰器本身是否引入了额外的同步开销？如何量化？
