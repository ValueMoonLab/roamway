# 受管终端（terminal）

## 命令

```bash
roamway terminal create [--name N] [--workspace DIR] [--shell sh|bash|cmd|powershell] [--buffer-kb N]
roamway terminal ls
roamway terminal capture <id> [--since N] [--tail N]
roamway terminal send-keys <id> <keys...> [--enter]
roamway terminal kill <id>
```

## 无 TTY 的限制

受管终端用「管道 + 滚动缓冲」实现，**不是 PTY**：

- 依赖 `isatty` 的 TUI 程序（`vim`、`htop`、`top` 等交互界面）不支持。
- 适合跑长命令、构建、日志类输出：`ls -la`、`make`、`npm run build` 没问题。
- `send-keys` 只能向进程 stdin 送文本，无法模拟完整终端按键矩阵。
- shell 白名单：`sh`、`bash`、`zsh`、`cmd`、`powershell` 等（见 `src/terminals.rs`）。

## 与工作区脚本的关系

`roamway script add --name N --cmd "..." [--workspace DIR]` 注册的是**常驻脚本**（`script ls/start/stop`），不是 PTY；`workspace setup <name>` 会在同名脚本优先时执行该脚本。