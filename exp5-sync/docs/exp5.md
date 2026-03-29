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

### 当前状态

当前 `exp5-sync` 已经调整为一个双模式 crate：

- `std` 模式：用于宿主机多线程实验、指标采集和测试
- `kernel` 模式：用于 `no_std + alloc` 内核环境，可直接作为 `ch8` 的同步库依赖

这意味着后续章节如果只依赖 `exp5-sync`，基础同步原语
`MutexBlocking / Semaphore / Condvar / UPIntrFreeCell`
已经可以独立工作，不再必须额外依赖 `tg-rcore-tutorial-sync`。

### 当前 syscall 组织

当前仓库已经统一到一套个人维护的 syscall crate：

- 内核侧：[`syscall-t3l8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/syscall-t3l8/Cargo.toml)
- 用户态：[`tg-rcore-tutorial-user`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-user/Cargo.toml)
  也已经改为依赖 `syscall-t3l8`

这意味着如果对外发布并长期维护，只需要维护你自己的
`syscall-t3l8` 即可，不必同时维护另一套通用 syscall crate。

### 真实替换验证

当前仓库已经完成了真实章节替换验证：

- [`tg-rcore-tutorial-ch8`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-ch8/Cargo.toml)
  已改为依赖 `exp5-sync` 的 `kernel` feature，替代原 `tg-sync`
- [`tg-rcore-tutorial-user`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/tg-rcore-tutorial-user/Cargo.toml)
  已改为统一依赖 `syscall-t3l8`
- `SpinLock` 已复用 `mutex_create(false)` 接入
- `RwLock` 已补齐 `rwlock_create/read_lock/write_lock/unlock` syscall 链路

验证命令：

```bash
cargo test --manifest-path exp5-sync/Cargo.toml --offline
cargo check --manifest-path exp5-sync/Cargo.toml --no-default-features --features kernel --offline
cargo check --manifest-path syscall-t3l8/Cargo.toml --features kernel --offline
cargo check --manifest-path tg-rcore-tutorial-user/Cargo.toml --offline
cargo check --manifest-path tg-rcore-tutorial-ch8/Cargo.toml --offline
```

这说明 `exp5-sync` 已经不是“接口兼容的实验 crate”，而是已经完成了
**替换原同步层 + 接通 syscall + 通过真实章节编译验证** 的版本。

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

当前基础同步原语和 syscall 已经可以直接在 `ch8` 中工作；若要继续扩展，还可以做：

1. **Metrics**：在 ch8 的 `main.rs` 调度循环中，对 `MUTEX_LOCK` / `SEMAPHORE_DOWN` / `RWLOCK_*` 等
   系统调用路径添加时间戳记录
2. **用户态样例**：在 `tg-rcore-tutorial-user/src/bin/` 新增 `rwlock` 示例程序
3. **反向测试**：通过用户态测试程序（`tg-rcore-tutorial-user/src/bin/`）实现

## 讨论题

1. 自旋锁和睡眠锁在什么场景下各有优势？临界区长度如何影响选择？
2. 读写锁相比互斥锁在"多读少写"场景下提升了多少吞吐？代价是什么？
3. 哲学家就餐中，有序获取和无序获取的区别体现在哪些指标上？
4. `InstrumentedMutex` 装饰器本身是否引入了额外的同步开销？如何量化？
