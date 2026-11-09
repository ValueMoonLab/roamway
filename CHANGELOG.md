# Changelog

## 1.0.0

首个对齐 Paseo 0.15.0 CLI 面的版本（31 个顶层命令 / 96 条）。

### Added

- 新增 `start`（= `daemon start`）、`send <agent-id> <prompt...>`、`delete <id> [--yes]`（运行中先 stop 再硬删）。
- 新增 `permit ls | approve <id> | deny <id>`（`ls` = `GET /v1/approvals`）。
- 新增 `heartbeat ls|create|update|delete`（薄封装 schedules；`--agent` 绑定 agent，`--cron`/`--every` 传 cron/every）。
- 新增 `workspace create <name> --path DIR`（服务端 `POST /v1/workspaces` 注册已有目录）、`workspace archive <name>`、`workspace setup <name>`。
- 新增 `project ls|delete` 的显式占位：打印 `project: not supported (no project model; use workspace ls)` 并 exit 2。
- 全局参数补齐：`-o/--format table|json|yaml`（`--json` 等价）、`-q/--quiet`、`--no-headers`、`--no-color`、`--home`（映射 `--data`）；自带极简 YAML 序列化器，无新增 crate 依赖。
- `roamway help <cmd>` 与 `roamway <cmd> --help` 输出同一份 per-command help。
- 文档：`docs/quickstart.md`、`docs/config.md`、`docs/providers.md`、`docs/plugins.md`、`docs/terminals.md`、`docs/webui.md`、`docs/troubleshooting.md`。

### Changed

- CLI 退出码契约：未知命令/未知子命令/缺少必填参数 → 2；网络失败 → 1；成功 → 0。`dispatch` 抽成返回退出码的纯函数。
- `--help/-h` 在任何副作用之前短路（`roamway run --help`、`roamway ls --help` 不再建 run / 查列表）。
- `README` 命令一览改为 `roamway help <cmd>` 指向；新增 Web 控制面说明（`/ui/`、`/ui/app.js`、token 用法、截图占位说明）。
- 重写 `PARITY.md`：31 顶层 / 65 叶子逐条四列（状态/证据/差异或不做理由），文末为 P1–P11 完成事实。
- `Cargo.toml` 版本 `0.1.0` → `1.0.0`（`roamway --version` 与 `/health` 同步）。

### Fixed

- `roamway delete|send|start|heartbeat|permit|project ...` 原先静默空转、exit 0，现已补齐实现或按不支持 exit 2。
- `roamway -o json ls` / `roamway --format json ls` 原先打总帮助，现已支持短参并输出 JSON。
- `roamway ls --help` 原先真的执行 ls，现已短路。
- `roamway bogusxx` 原先 exit 0，现改为 exit 2。