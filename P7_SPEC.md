# P7 规范：终端与工作区脚本（PARITY P7，10 条命令）

Paseo：`terminal ls/create/kill/capture/send-keys` + `script ls/start/stop`。

roamway 这层用「受管长驻进程」实现：不用 PTY（std-only 实现 PTY 需要平台特判），但要有**持久 stdin、滚动缓冲、可抓取输出、可杀**。

## 1. 数据模型（`src/terminals.rs`）

```rust
pub struct Terminal {
    pub id: String,          // term_ + 8 hex
    pub name: String,        // 默认 shell-<n>
    pub workspace: String,
    pub shell: String,       // bash | sh | pwsh | cmd（默认按平台）
    pub pid: Option<u32>,
    pub state: String,       // running | exited
    pub created_at: i64,
    pub exit_code: Option<i32>,
    pub bytes_out: u64,
}
pub struct Script { pub id: String, pub name: String, pub cmd: String, pub workspace: String, pub state: String, pub pid: Option<u32>, pub last_exit: Option<i32>, pub started_at: i64 }
```

- 运行时状态放内存 `HashMap<String, TerminalRuntime>`，元数据落 `terminals.json` / `scripts.json`（重启后标 `exited`，不自动复活）
- 每个终端保留最近 **256 KB** 输出环形缓冲（超出丢头部），`capture --since <offset>` 支持增量读取

## 2. 命令

| 命令 | 行为 |
|---|---|
| `terminal create [--name --workspace --shell]` | 起长驻 shell，返回 id/pid |
| `terminal ls [--json]` | ID NAME STATE PID BYTES WORKSPACE |
| `terminal capture <id> [--since N] [--tail N]` | 输出缓冲内容（默认从头，`--tail` 取尾部） |
| `terminal send-keys <id> <keys...>` | 把参数按空格连接后写入 stdin；支持 `\n` 尾随与 `--enter` |
| `terminal kill <id>` | 杀进程树（复用 `kill_tree`），标 exited |
| `script ls [--json]` | 列出配置的脚本 |
| `script start <name>` | 在指定 workspace 起脚本（长驻），记录 pid |
| `script stop <name>` | 停脚本 |
| `script add --name N --cmd "..." --workspace P` | 注册脚本（Paseo 里由配置文件定义，roamway 给命令入口） |
| `script rm <name>` | 删除注册 |

## 3. HTTP 路由

| 方法 | 路径 |
|---|---|
| GET/POST | `/v1/terminals` |
| GET/DELETE | `/v1/terminals/{id}` |
| GET | `/v1/terminals/{id}/output?since=N` |
| POST | `/v1/terminals/{id}/keys` `{"keys": "..."}` |
| GET/POST | `/v1/scripts` |
| POST | `/v1/scripts/{name}/start` `/stop` |

## 4. 实现要点

- 子进程：`Command::new(shell).arg("-c").arg("exec bash -i 2>&1")`（Unix）/ `cmd.exe /Q`（Windows）；stdin/stdout/stderr 全接管
- 读线程：`BufReader` 逐行读，追加到环形缓冲，更新 `bytes_out`
- 写：`ChildStdin::write_all` + `flush`；进程已退出返回明确错误
- `kill`：先 SIGTERM，2 秒后 SIGKILL（Windows `taskkill /T /F`）
- 缓冲区上限可配 `--buffer-kb`（默认 256）

## 5. 单测（≥8）

环形缓冲截断、`capture --since` 偏移、`send-keys --enter` 补换行、终端不存在报错、脚本重名拒绝、进程退出后 state=exited、`kill` 幂等、`bytes_out` 累计。

## 6. 验收（NAS）

1. `terminal create` → `send-keys "echo hi" --enter` → 2 秒后 `capture` 出现 `hi`
2. 连续发 3 条命令，`capture --since 0` 包含全部三条
3. `terminal kill` 后 `terminal ls` 显示 `exited`，进程表无残留
4. `script add --name build --cmd "sleep 30" --workspace /home/roamway` → `script start` → `script ls` running → `script stop` → 进程消失
5. 重启 daemon 后终端标 `exited`，脚本不自动复活
6. 8 个单测全过，`cargo build --release` 0 warning

## 7. 约束

- 不使用 PTY，文档里如实写明"非交互式终端（无 TTY），TUI 程序不支持"
- 所有命令支持 `--json` 与 `--host`
- Windows 分支可编译
