## ch3
- Q: 我现在要完成 ch3, 请解释 ch3 需要完成什么功能，实现代码并解释你是怎么实现的
- A: Ch3 要做什么（功能层面）
基础（README DoD）
多任务 TCB、轮转调度、时钟中断抢占（默认）与 yield 协作式（--features coop）、clock_gettime 等——这些在仓库里本来就有。

练习（exercise.md）
增加 sys_trace（号 410），按 trace_request 做三件事：

0：把 id 当 *const u8，读一字节并返回；
1：把 id 当 *mut u8，把 data 的最低字节写入；
2：查询「系统调用号 = id」在本任务内的调用次数，且当前这次 TRACE 也要先算进统计；
其它 trace_request → 返回 -1。
实现思路（为何写在 task.rs）
调用次数必须是每个任务一份，放在 TaskControlBlock 里最自然。
Trace for SyscallContext 拿不到当前 TaskControlBlock，若在 main.rs 里用全局表还要再传任务下标。
因此在 handle_syscall 里在进入 tg_syscall::handle 之前：
用 id.0 作为下标，在 syscall_counts 里 +1（含 TRACE）；
若 id == TRACE，不再走 tg_syscall::handle，在本函数内按上表处理 trace_request，写回 a0 并 move_next，返回 SchedulingEvent::None；
其它系统调用仍走原来的 tg_syscall::handle。
这样 trace_request == 2 时，syscall_counts[TRACE] 已包含当前这次 trace，查询 TRACE 次数会多 1，符合题意。

统计表长度取 512，覆盖 410 等号；更大下标在查询时返回 0。

代码改动位置
task.rs：syscall_counts、SYSCALL_COUNT_MAX、init 里清零、handle_syscall 里计数 + TRACE 分支。
main.rs：Trace 实现改为说明性注释 + 仍注册 init_trace（实际路径不会调到它）。
你本地若刚才跳过了 cargo build，可在 tg-rcore-tutorial-ch3 下执行：

cargo build --target riscv64gc-unknown-none-elf
./test.sh exercise   # 需已安装 tg-rcore-tutorial-checker
验证通过即表示练习侧 sys_trace 与统计逻辑与测例一致。若你希望把实现说明写进 docs/report.md，我可以按你报告结构改一版短文。

Q: 改成动态表，只有调用了的syscall 才计算 counts
A: 已改成按任务一张稀疏表，只在某系统调用号第一次出现时插入一条 (nr, count)，之后只递增 count，不再为未使用的号占空间。

实现要点（exercise 下 syscall_trace）
每个任务：len + 最多 MAX_DISTINCT = 64 条 Entry { nr: u32, count: u32 }，仍在 BSS 的 TABLES[task] 里，不放进 TCB、不碰内核栈。
bump：在已有条目里找 nr，找到则 count + 1；否则在未满时追加 count = 1。
get：线性查找，没有则返回 0（与「从未调用」一致）。
clear_row：只把 len = 0，不必清零整段 entries。
与真·堆上动态分配的区别
当前 ch3 没有全局分配器，不能做无界 Vec/HashMap。这里的「动态」是：有界稀疏表，只存出现过的号，空间从原来的约 128KiB（512×32×usize）降到约 32×(64×8+α) ≈ 16KiB 量级。

若某个测例单任务用到的不同系统调用号超过 64，多出来的号不会再记入（一般不会发生在 ch3 练习里）。若要放宽，只要把 MAX_DISTINCT 调大即可。


## ch4
Q: 我现在要完成 ch4, 请解释 ch4 需要完成什么功能，实现代码并解释你是怎么实现的
A: Ch4 需要完成什么
第四章引入了 Sv39 虚拟内存，每个进程有独立地址空间。练习有三项：

练习	要点
重写 sys_trace	ch3 直接解引用用户指针，现在必须先用 translate() 翻译虚拟地址并检查权限（读需 U_RV，写需 U_WV），不合法返回 -1
实现 mmap	分配物理页映射到用户地址空间，需校验 addr 页对齐、prot 合法性、目标区间无重叠
实现 munmap	取消映射，需校验 addr 页对齐、目标区间全部已映射
实现说明
1. sys_trace（Trace for SyscallContext）
与 ch3 不同，用户传入的 id（地址）是虚拟地址，内核无法直接解引用。改用 process.address_space.translate::<u8>(VAddr::new(id), flags) 做地址翻译 + 权限检查：

request=0（读）：用 build_flags("U_RV") 检查可读，成功则返回该字节值，否则返回 -1。
request=1（写）：用 build_flags("U_WV") 检查可写，成功则写入 data as u8 并返回 0，否则返回 -1。
request=2（计数）：ch4 不需要维护调用计数，直接返回 0。
2. mmap
fn mmap(&self, caller, addr, len, prot, ...) -> isize
参数校验：

addr 必须 4KiB 对齐
prot & !0x7 != 0 或 prot & 0x7 == 0 → 返回 -1
遍历 process.address_space.areas 检查目标 VPN 范围无重叠
权限映射（prot → RISC-V PTE flags）：

prot bit0 (R) → R，prot bit1 (W) → W，prot bit2 (X) → X
始终加 U（用户态可访问）和 V（有效）
调用 address_space.map(start_vpn..end_vpn, &[], 0, flags) 分配物理页并建立映射。

3. munmap
fn munmap(&self, caller, addr, len) -> isize
addr 必须页对齐
逐页检查目标范围内每一页都落在某个 areas 区间内，否则返回 -1
调用 address_space.unmap(start_vpn..end_vpn) 清除页表项并从 areas 移除
验证方式
在虚拟机上：

cd tg-rcore-tutorial/tg-rcore-tutorial-ch4
cargo run                        # 基础测试
cargo run --features exercise    # 练习测试
./test.sh exercise               # 自动化检查