# Changelog

## 1.3.0

数据目录单写者与 store 健康可观测（P26）：同一个 `<data>` 只允许一个写者，降级启动对外可见，
且降级时不覆盖最后一份可用备份。

### Added

- `serve` 单写者锁 `<data>/roamway.lock`：当前进程正服务同一目录时，第二个 `serve` 直接退出
  码 3；`--force` 显式接管（打印「two writers will corrupt state.json」警告）。
- `StoreHealth` 四值（`fresh` / `ok` / `recovered` / `unrecoverable`）：`/readyz` 追加 `"store"`
  字段，`recovered` 仍 200，`unrecoverable` 回 503 `state_unreadable`；`/healthz` 不变。
- `roamway daemon status` JSON 增加 `"store"` 与 `"writer"`；文本模式多一行
  `store ... writer pid ... bind ...`，daemon 离线也直接本地体检 `state.json`。

### Changed

- 降级启动时把可用的 `.bak` 复制为 `state.json.bak.known-good`（已存在不覆盖），并让降级后的
  首笔 `save()` 跳过 `.bak` 轮换，保住最后一份已知良好备份。
- 坏文件隔离快照 `state.json.corrupt-*` 只保留最近 5 份，避免反复崩溃无限堆积。

## 1.2.0

探活可分性、版本定版、usage 面收口（P25），叠加无人值守运行面的第二段（P21–P24）：只读 Web 面板、
时间维度保留、本地快照 `backup`/`restore`、写盘去重与写计数单一真源、pid 文件生命周期，
以及本地 CLI 的 421 修复。

### Added

- `GET /healthz`（liveness）：免鉴权，**不取 store 锁**，只读启动时间与编译期版本，回
  `{"ok":true,"version":...,"uptime_s":...}` 200。state 锁被一次大 `state.json` 落盘持有时，
  探针不再把"忙"读成"死"。
- `GET /readyz`（readiness）：免鉴权，用 `try_lock`；拿不到锁回 `503 {"ok":false,"reason":"busy"}`，
  拿到锁回 200。忙 ≠ 死是这条的全部意义。
- `/healthz`、`/readyz` 只接受 GET，其他方法一律 405（不是 404）——编排器能把"方法错了"与
  "路由不存在"分开。
- `hosts::endpoint_is_wildcard()` 与 `roamway host probe <name>` 的提示：endpoint 是 `0.0.0.0`/`::`/`*`
  这类 listen 地址时，probe 之后额外提示"远端访问请写实际主机名/IP"，不再让人照着 421 段反复试。
- `keep_days`（第 14 个 daemon 配置键，默认 `0`=不限）与 `prune --max-age-days N`：
  保留判定仍是 `config::retention_plan_full` 一处，按条数（`keep_runs`）与按时间（`keep_days`）
  两条规则共用同一份"只删 terminal run"的逻辑；`prune` 的报告分列 `removed` 与 `age_removed`。
- `roamway backup [--out DIR] [--keep N]`：把 `state.json`/`config.json`/`schedules.json`
  复制成 `roamway-backup-<YYYYmmdd-HHMMSS>/` 并写 `manifest.json`（逐文件 `bytes` + `sha256`）；
  token/pairing/passwords 等凭据文件永不进快照。
- `roamway restore --from DIR [--data DIR] [--force] [--yes]`：先校验 `format` 与每个文件的
  大小/哈希，任一不符则一个字节都不写；写前把现有文件轮换为 `*.pre-restore-<时间戳>`。
- daemon 存活检测三证据：`<data>/roamway.pid` 的进程存活、`<data>/heartbeat` 60 秒内有更新、
  `<data>/bind` 指向的端口可连接；任一命中且未 `--force` 时 `restore` 返回 exit 3。
- `/metrics` 新增 `roamway_state_writes_skipped_total`（内容未变而跳过的落盘次数）与
  `roamway_data_bytes`（`<data>` 顶层文件 + `logs/*.log` 字节数）。
- `<data>/roamway.pid` 由 serve 自己维护：启动时写、退出时仅在 owner 仍是本进程时删。

### Changed

- `Store::save` 在序列化字节与上一次写入完全相同时跳过落盘（`last_write_hash`），只累加
  `writes_skipped`，NAS 上的无谓写放大与 mtime 抖动随之消失。
- 写计数改为 `Store` 内单一真源（`writes_total`/`writes_skipped`），`/metrics` 在已持 store 锁的
  代码块内读取，不再依赖调用点手动累加；计数仅内存态，重新 `load` 归零。
- 版本定版为 `1.2.0`：`Cargo.toml` 与 CHANGELOG 同步；`roamway --version`、`/health`、
  `/v1/health` 继续统一取 `env!("CARGO_PKG_VERSION")`，不引入第二处硬编码版本常量。

### Fixed

- **本地 CLI 的 `421 Misdirected Request`**：daemon 绑 wildcard 地址（`0.0.0.0`/`::`/`*`/空 host）时，
  不带 `--host` 的命令会把 `Host: 0.0.0.0` 发给服务端，被 Host 白名单判 421。新增
  `hosts::client_base_from_bind` 把 wildcard 监听地址映射为 `127.0.0.1`（保留 scheme/port/path，
  IPv6 方括号正确解析），具体的 `127.0.0.1`/`localhost`/`[::1]`/`https://…` 原样保留；
  `main::local_base` 改走它。
- `backup`/`prune`/`restore` 的 usage 串统一由 `USAGE_*` 常量出（帮助文本、dispatch、usage_error
  三处同源），全部带 `[--data DIR]`；此前照 help 传参会 exit 2。
- Windows CI：auth TTL 断言放宽到 60s、`tasklist` 输出改按 CSV 字段匹配 pid（本地化列名无关）。

## 1.1.0

运行面、参数面与运维面对齐 Paseo 的第二轮（P12–P20）：`ls`/`run` 语义、provider 会话续跑、
daemon 持久配置、provider 参数透传、worktree 选项、`/metrics`、job 依赖，以及 Windows 原生
预编译产物与 Windows 端到端冒烟。

### Added

- `run` 透传 Paseo 的 provider 参数：`--thinking`、`--mode`、`--provider`、`--output-schema`、
  `--image`（可重复，校验本地图片路径存在）；不支持的组合在执行前 exit 2，不再静默忽略。
- `run --env K=V`（可重复，校验 key 合法性）、`run --cwd DIR`、`run --label k=v`。
- worktree 参数收口：`--worktree-slug`、`--new-branch`、`--base`、`--branch` 与
  `--worktree-mode branch-off|checkout-branch|checkout-pr` + `--pr-number` 组合校验；worktree 路径按 slug 命名并在
  agent archive 时按 run 上记录的路径反查清理。
- provider 会话续跑：`run --resume` / `run --continue` 续用 `agent.session_id`；
  `GET /v1/agents/{id}/session` 暴露会话 id；不可续跑的 agent kind（shell、`api:*`）在 `run.resume_note`
  写 `resume unsupported`，不再靠 `agent.last_error` 兜底。
- daemon 部署面：持久 `config.json`（12 个键）+ `daemon config show|get|set|unset`，
  优先级 CLI > `ROAMWAY_*` 环境变量 > 文件 > 默认；`daemon run` 前台入口与 `daemon start` 共用同一套配置解析，
  token 只存环境变量名、永不落盘。
- `GET /metrics`（Prometheus 文本格式）：进程/运行/任务/连接计数，静态文本走 `respond_static`，不占 JSON 路由。
- job 依赖：`Job.depends_on` + `Job.last_skip`；依赖未完成时调度器跳过并记录等待原因；
  `POST /v1/jobs` 校验 cycle 与未知 id；CLI `jobs-add --after <id>`（可重复），`jobs` 表新增 `AFTER` 列，
  `jobs-run` 显式绕过依赖并标 `deps_bypassed`。
- 全局 `/metrics` 与 job 依赖进 README Roadmap；`.github/workflows/release.yml`：
  推 `v*` tag 自动构建三平台产物（`x86_64-unknown-linux-gnu`、`aarch64-apple-darwin`、
  `x86_64-pc-windows-msvc`），`gh release create` 用 `secrets.GITHUB_TOKEN` 建 Release，
  notes 取本文件第一个 `## ` 段。
- `scripts/smoke.ps1`：Windows 端到端冒烟（auth 401 / shell run / 2 秒调度 / stop→killed），
  CI 的 `windows-latest` job 与 Release 包自检共用。

### Changed

- `roamway ls` 默认 agent 维度（`ID/STATE/KIND/MODEL/TITLE/UPDATED`），`--runs` 才看 run 表；
  agent 维度支持 `--label/--limit/--kind/--global`，`-q` 输出每行一个 id。
- CLI 参数解析硬化：未知短标志一律 usage error（exit 2），不再被当成 prompt 前缀；
  `-i` 为 `--image` 别名；`ls --label` 一律要求 `k=v`，`ls --runs --label` 显式 exit 2（run 列表无 label 过滤）。
- `roamway --version` 与 `/health` 的 version 同步到 `1.1.0`。

### Fixed

- `run --thinking/--mode/--image/...` 原先一律 `not supported` 直接 exit 2，现按 provider 能力透传或明确报错。
- `ls --runs --label k=v` 原先静默返回全部 run，现 exit 2 并说明原因。
- job 计数把 `/ui/` 的 404 误算进 `failed_total`，现单列 `UiResponse::NotFound`。

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
