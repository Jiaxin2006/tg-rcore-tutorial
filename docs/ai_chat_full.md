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

Q: 请直接从 ch3 merge syscall count
A: 已按 ch3 的做法改完：syscall 统计不再挂在每个 Process 的大数组上，而是用 BSS 里的稀疏表（与 ch3 task.rs 里 syscall_trace 同结构），并只在 --features exercise 下编译与更新。

改动摘要
process.rs

增加 #[cfg(feature = "exercise")] pub(crate) mod syscall_trace：clear_row / bump / get，TABLES: [TaskTable; MAX_PROCESS_SLOTS]（32 槽，每槽最多 64 种不同 syscall 号）。
Process 增加 task_slot: usize（创建时用应用枚举下标 i，与 ch3 的 task_index 一致）。
Process::new(elf, task_slot)：在 exercise 下对新槽 clear_row(task_slot)。
main.rs

Process::new(..., i) 传入枚举下标 i。
在 schedule 里、tg_syscall::handle 之前（仅 exercise）：syscall_trace::bump(当前进程.task_slot, id.0)。
trace request=2（仅 exercise）：syscall_trace::get(process.task_slot, id)；非 exercise 仍为 0。
这样 不会在每进程结构里塞 512×usize，工作集小得多，行为与 ch3 的统计语义一致（含「本次 TRACE 先计入再查询」）。

请在本机验证：

cd tg-rcore-tutorial-ch4
cargo run --features exercise

Q: 请帮我总结一下和 ch3 一样（T1L1 分支）打包上传 crate 的步骤
A: 完整步骤
第一步：改 Cargo.toml
和 ch3 一样，只改 [package] 元信息 + keywords + 加 [[bin]]；不用动 path 依赖（发布时 Cargo 自动忽略 path，只用 version）。

Cargo.toml
+12
-8
[package]name = "tg-rcore-tutorial-ch4"description = "Chapter 4 of rCore Tutorial: Address space management with SV39 virtual memory."version = "0.4.8"
name = "tg-rcore-tutorial-ch4"description = "Chapter 4 of rCore Tutorial: Address space management with SV39 virtual memory."version = "0.4.8"[package]name = "jiaxin2006-tg-rcore-tutorial-t1l2"
第二步到第六步（在虚拟机上执行）
按顺序执行：

# ── 1. 确认测试通过 ──
cd /home/parallels/Desktop/OS/tg-rcore-tutorial/tg-rcore-tutorial-ch4
cargo run                        # 基础测试
cargo run --features exercise    # 练习测试
# ── 2. 打包验证 ──
cargo package --list             # 看包含哪些文件
cargo package                    # 生成 .crate 并本地验证编译
cargo publish --dry-run          # 模拟上传
# ── 3. 正式发布 ──
cargo login                      # 若本次会话尚未登录
cargo publish
# ── 4. 提交 + 打 tag ──
cd /home/parallels/Desktop/OS/tg-rcore-tutorial
git add -A
git commit -m "ch4: implement trace/mmap/munmap exercises, prepare crate for publishing"
git tag jiaxin2006-tg-rcore-tutorial-t1l2-v0.0.1-preview.1
git push origin T1L2
git push origin jiaxin2006-tg-rcore-tutorial-t1l2-v0.0.1-preview.1
# ── 5. 合并到 test ──
git checkout test
git pull origin test
git merge T1L2
git push origin test
# ── 6.（可选）建下一章工作分支 ──
git checkout test
git checkout -b T1L3
git push -u origin T1L3
课程表格填写
3.21: jiaxin2006-tg-rcore-tutorial-t1l2
（日期改成你实际发布的那天。）

复现方式（助教可用）
# 方式一：crates.io
cargo clone jiaxin2006-tg-rcore-tutorial-t1l2
cd jiaxin2006-tg-rcore-tutorial-t1l2
cargo run --features exercise
# 方式二：GitHub
git clone https://github.com/Jiaxin2006/tg-rcore-tutorial.git
cd tg-rcore-tutorial/tg-rcore-tutorial-ch4
cargo run --features exercise
流程和 ch3 完全一致，唯一区别是包名从 t1l1 变成了 t1l2、tag 名对应更新。

## ch5

Q:1. 现在能确定 ch4/ch3 已经完成的部分同步到 T1L3 分支了吗？
2. 我要完成 ch5, 请帮我解释这里都需要做什么，并实现代码
A: 可以，先直接回答你的两个问题：

ch4/ch3 是否同步到 T1L3
目前仓库在 T1L3，且 T1L3 与 origin/T1L3 指向同一提交 ff6ab8a（无 ahead/behind）。
这个提交就是你的 ch4 完成提交，并且历史里包含了 ch3 的提交 7d5ac6a。
所以就“分支同步”这件事，当前是已同步的。
ch5 要做什么 + 我已经帮你实现了什么
ch5 exercise 核心要点是 4 件事：

迁移 mmap / munmap 到新的进程结构（从当前进程地址空间做映射/取消映射）
实现 spawn（ID 400）：直接从 ELF 创建子进程，不走 fork 地址空间拷贝
实现 set_priority（ID 140）：优先级必须 >=2
将调度从 FIFO 改为 stride：每次选 stride 最小者运行，运行后 stride += BigStride / priority
我已在 ch5 里完成实现：

tg-rcore-tutorial-ch5/src/process.rs

给 Process 增加 stride、priority
新建进程（from_elf / fork）初始化为 stride=0、priority=16
tg-rcore-tutorial-ch5/src/processor.rs

调度器从 FIFO 改为 stride 选择最小 stride 的 PID
每次被选中后更新 stride += pass，pass = BIG_STRIDE / priority（并做了最小值保护）
tg-rcore-tutorial-ch5/src/main.rs

实现 spawn：从用户传入路径解析 app，创建新进程并挂到当前进程子树
实现 set_priority：prio < 2 返回 -1，否则设置并返回 prio
实现 mmap / munmap：页对齐校验、prot 校验、区间冲突/覆盖校验、映射与反映射
验证结果：

我跑过 TG_SKIP_USER_APPS=1 cargo check --features exercise，ch5 已通过编译。
直接 cargo check --features exercise 会因你本机缺少 cargo-clone 在 build.rs 阶段失败（这是环境问题，不是这次代码逻辑问题）。
你下一步可以这样测：

安装：cargo install cargo-clone
然后在 tg-rcore-tutorial-ch5 下跑：./test.sh exercise
如果你愿意，我下一步可以继续帮你直接跑 exercise 测例并按失败项逐个修到全绿。

## ch6

Q: 请帮我查看 ch6 的要求并实现代码。查看 @tg-rcore-tutorial/docs/report.md 并吸取之前的bug 的教训
A: 我先按你的要求做两步：先读 docs/report.md 提炼“之前踩坑要避免什么”，再完整扫描 ch6 的 exercise.md 和代码里的 TODO，然后直接实现并编译验证。

我已经定位到关键点：ch6 不只是三个文件系统 syscall，还要补上 ch5 遗留 TODO（spawn/priority/mmap/munmap/stride），否则 ch6_usertest 里的前向兼容测例会挂。
我先从 easy-fs 底层补齐硬链接元数据（nlink）和 inode 回收，再接 syscall 层。

layout.rs
+6
    pub size: u32,    pub direct: [u32; INODE_DIRECT_COUNT],        self.size = 0;
    pub size: u32,    pub nlink: u32,    pub direct: [u32; INODE_DIRECT_COUNT],        self.size = 0;        self.nlink = 1;
efs.rs
+5
    }
    }    /// Deallocate an inode    pub fn dealloc_inode(&mut self, inode_id: u32) {        self.inode_bitmap            .dealloc(&self.block_device, inode_id as usize);
vfs.rs
+118
pub struct Inode {    block_id: usize,    pub fn new(
pub struct Inode {    inode_id: u32,    block_id: usize,    pub fn new(        inode_id: u32,
efs.rs
+1
-1
        // release efs lock        Inode::new(block_id, block_offset, Arc::clone(efs), block_device)    }
        Inode::new(block_id, block_offset, Arc::clone(efs), block_device)        // release efs lock        Inode::new(0, block_id, block_offset, Arc::clone(efs), block_device)    }
fs.rs
+4
-4
    /// 创建硬链接（TODO 练习题）    fn link(&self, _src: &str, _dst: &str) -> isize {        unimplemented!()
    fn link(&self, _src: &str, _dst: &str) -> isize {        unimplemented!()    /// 创建硬链接（TODO 练习题）    fn link(&self, src: &str, dst: &str) -> isize {        self.root.link(src, dst)
process.rs
+8
    pub program_brk: usize,
    pub program_brk: usize,    /// stride 调度算法中的当前步长    pub stride: usize,    /// 进程优先级（>= 2）    pub priority: usize,
processor.rs
+29
-2
/// 处理器全局管理器
/// stride 调度的大步长常量const BIG_STRIDE: usize = 1 << 20;/// 处理器全局管理器
我正在改 ch6/src/main.rs 的 syscall 实现，会特别按你 report.md 的经验避开两类坑：

显式类型标注（避免 SyscallId/返回值推断歧义）
unsafe 指针运算全部加清晰括号和局部变量（避免你提到的括号 bug）。
main.rs
+79
-29
    use crate::{        build_flags,        fs::{read_all, FS},
        build_flags,    use crate::{        build_flags, parse_flags,        fs::{read_all, FS},
main.rs
+105
-24
        /// spawn 系统调用（TODO 练习题）        fn spawn(&self, _caller: Caller, _path: usize, _count: usize) -> isize {            let current = PROCESSOR.get_mut().current().unwrap();            tg_console::log::info!(
        /// spawn 系统调用（TODO 练习题）        fn spawn(&self, _caller: Caller, _path: usize, _count: usize) -> isize {            let current = PROCESSOR.get_mut().current().unwrap();            tg_console::log::info!(                "spawn: parent pid = {}, not implemented",                current.pid.get_usize()            );            -1        }
代码改动已经到位，我现在开始跑 ch6 编译检查；如果有 borrow/type 错误会继续就地修完再给你结果。

