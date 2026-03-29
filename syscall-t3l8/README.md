# jiaxin2006-tg-rcore-tutorial-syscall-t3l8

本 crate 是 **个人作业用** 的 syscall 定义与分发层，crate 名与版本 **独立于** 老师发布的 `tg-rcore-tutorial-syscall`，避免在 crates.io 上与老师教程包冲突。

在老师仓库的 `tg-rcore-tutorial-syscall` 基础上扩展了（示例）：

- 帧缓冲相关：`Framebuffer` trait、`init_framebuffer`、`fb_get_info` / `fb_present`（及对应 syscall 号）
- 输入：`IO::input_getchar`（非阻塞 UART 读）
- 文件：`IO::lseek`

本地开发可与仓库内 `tg-rcore-tutorial-syscall` 并存；**第八章内核**应依赖本包以便 `cargo publish` 时从 registry 解析到个人 crate，而不是覆盖老师的 `tg-rcore-tutorial-syscall@0.4.8`。

依赖 `tg-rcore-tutorial-signal-defs` 与老师 crates.io 上已发布版本号对齐，仅增加本仓库中的扩展 syscall，不修改 signal-defs 包名。
