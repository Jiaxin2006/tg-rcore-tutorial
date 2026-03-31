# T4 Phase 0：基线冻结与测量口径

## 1. 目标

Phase 0 的目标不是修改内核功能，而是先把后续对比所需的“单核基线”固定下来，避免后面出现：

- 改了中断路径后，不知道是否引入回归
- 开了多核后，不知道性能提升来自哪里
- 不同时间测出来的数据口径不一致

因此，Phase 0 需要先完成两件事：

1. 固定当前 `tg-rcore-tutorial-ch8 + doom` 的基线状态
2. 固定后续所有 T4 测量都要复用的命令、指标和记录格式

## 2. 本阶段产物

当前仓库里，Phase 0 的落地产物有两项：

- [`scripts/t4-phase0-baseline.sh`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/scripts/t4-phase0-baseline.sh)
- [`docs/t4-phase0-baseline.md`](/Users/hanjiaxin/Desktop/操作系统/tg-rcore-tutorial/docs/t4-phase0-baseline.md)

脚本负责：

- 记录环境信息
- 记录 git 基线信息
- 运行 `ch8` 的轻量检查
- 运行 `ch8` 的 base / exercise 测试
- 自动记录 shell 启动、`race_adder_mutex_blocking`、`doomgeneric` 的基线指标
- 生成一份可填写的测量模板

## 3. 统一基线口径

后续 T4 对比默认以“单核、当前 ch8 实现”为基线。

建议固定的口径如下：

- 基线内核：当前 `tg-rcore-tutorial-ch8`
- 调度层：当前默认 `exp4::DefaultTaskManager`
- 同步层：当前 `exp5-sync` 的 `kernel` feature
- CPU 配置：单核
- 构建模式：`debug`
- 用户态镜像：使用当前 `build.rs` 自动打包的 `tg-rcore-tutorial-user`

如果后面切换成：

- 多核
- release
- 并行 Doom
- 新的用户态 benchmark

都必须明确写成“对照组之外的新配置”，不能混在 Phase 0 基线里。

## 4. 建议采集的基线项目

### 4.1 环境基线

至少记录：

- 日期时间
- git commit
- git working tree 是否干净
- `rustc` / `cargo` / `qemu-system-riscv64` 版本
- 宿主机信息

### 4.2 功能基线

至少记录：

- `cargo check --manifest-path tg-rcore-tutorial-ch8/Cargo.toml --offline`
- `./test.sh base`
- `./test.sh exercise`

### 4.3 图形路径基线

至少记录：

- Doom 是否可启动
- Doom 当前是否是 interactive 还是 `-playdemo demo1`
- Doom 首帧是否成功提交

说明：

- 当前 Doom 入口在非 interactive 模式下会自动使用 `-playdemo demo1`
- 这很适合作为后续 T4 的“可重复图形负载”基线

### 4.4 性能基线

Phase 0 不强求把所有数字一次测全，但建议先把表格和口径准备好。

推荐优先记录：

- Shell 启动是否正常
- Doom 首帧出现时间
- Doom demo 模式下主观流畅度 / 是否稳定运行
- `race_adder_mutex_blocking` 的总耗时

## 5. 使用方法

在仓库根目录执行：

```bash
bash scripts/t4-phase0-baseline.sh metadata
bash scripts/t4-phase0-baseline.sh check
bash scripts/t4-phase0-baseline.sh base
bash scripts/t4-phase0-baseline.sh exercise
```

如果希望一次性做完自动部分：

```bash
bash scripts/t4-phase0-baseline.sh all
```

脚本输出默认放在：

```text
target/t4-phase0/<timestamp>/
```

典型输出包括：

- `env.txt`
- `git-status.txt`
- `check.log`
- `test-base.log`
- `test-exercise.log`
- `baseline-template.md`

## 6. 手工测量建议

自动脚本主要负责“可脚本化”的部分；Doom 的启动与首帧时间已经自动记录，但图形正确性仍建议人工补充。

### 6.1 `fb_demo`

建议步骤：

1. 运行 `cargo run`
2. 在 shell 中执行 `fb_demo`
3. 记录：
   - 是否打印 framebuffer 信息
   - 是否看到正确画面
   - 从执行到画面出现的大致时间

### 6.2 Doom

建议步骤：

1. 确认 `doomgeneric` 和 `doom1.wad` 已打进镜像
2. 运行 `cargo run`
3. 在 shell 中执行 `doomgeneric`
4. 记录：
   - 是否成功启动
   - 当前是 interactive 还是 demo 模式
   - 首帧时间
   - 是否出现明显卡死、黑屏、输入异常

## 7. 建议记录模板

后续每次做大改动前后，都建议按同样格式记录：

```text
版本标识：
单核 / 多核：
中断策略：
构建模式：

check:
base test:
exercise test:

doom:

并发测试耗时：
备注：
```

## 8. 与后续 Phase 的关系

Phase 0 完成后，后面的 Phase 1~6 都应复用这套基线口径：

- Phase 1 比较“内核态中断响应前后”的正确性与延迟
- Phase 2/3 比较“单核 vs 多核”的功能差异
- Phase 6 比较“复杂应用在 1 核 / 2 核 / 4 核”下的性能变化

如果没有 Phase 0，这些比较会很难解释。
