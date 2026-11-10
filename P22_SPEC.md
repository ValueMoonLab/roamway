# P22 — 长跑运维硬化：保留策略单一真源 + 日志封顶 + `prune` + 字节指标

## 背景与已勘事实（实现前请自行复核行号）

roamway 部署在家庭 NAS 上长期无人值守。P20 之后暴露出的运维缺口，都是"跑得越久越糟"：

1. **保留阈值有两套，且运行期不读配置**
   - `src/config.rs:549 fn trim_runs(runs: &mut Vec<Run>, keep_runs: usize) -> bool`：按 `keep_runs` 修剪，`0` = 不修剪。
   - 唯一调用点 `src/main.rs:1226`（daemon 启动时一次性执行）。默认值 `src/config.rs:510` 为 `500`。
   - `src/jobs.rs:246 fn prune(st: &mut Store)`：**硬编码 `MAX_RUNS: usize = 800`**，完全忽略 `keep_runs`。
   - `prune` 调用点：`src/jobs.rs:207`（job 触发）、`src/schedules.rs:509`（schedule 触发）、`src/server.rs:2698`（`POST /v1/runs`）。
   - 结果：`keep_runs=100` 的用户在运行期仍然膨胀到 800 条，重启一次又掉到 100 条 —— 阈值随生命周期抖动。

2. **单 run 日志无上限**
   - 日志文件路径 `src/types.rs:321 fn Store::log_path`（`<data>/logs/<run_id>.log`）。
   - 写入点：`src/runner.rs:79`（`File::create`）、`src/runner.rs:149`（`OpenOptions::append`）、`src/runner.rs:746`（`run_cli` 追加）。没有任何字节上限。
   - 日志只在 run 被删除/被 prune 时 `remove_file`（`src/jobs.rs:263`、`src/server.rs:2809`）。一个刷屏的 agent 能写满 NAS 磁盘。

3. **没有手动修剪入口**：`src/main.rs:1006 fn dispatch` 的命令表里没有 `prune`；`src/server.rs:1270 fn route` 里没有 `/v1/prune`。

4. **`/metrics` 没有任何字节类 gauge**：`src/server.rs:466` 起只有 build_info / uptime / runs / agents / jobs / schedules / terminals / conns 等计数。

5. **`state.json` 是全量 pretty 序列化**：`src/types.rs:292 fn Store::save` 每次把整个 `State`（含全部 runs/agents）写盘。runs 越多，每次状态变更的写盘成本和 COW 抖动越大。

## 目标

让 daemon 在无人值守下**磁盘占用有界、阈值只有一个真源、可手动回收且有指标可看**。不新增 crate 依赖（保持零依赖策略）。

## 一、保留阈值单一真源

- 删除 `src/jobs.rs` 的 `MAX_RUNS` 常量与现行 `prune` 语义分叉，改为：

  ```rust
  /// 与启动期同一套阈值：keep_runs=0 表示不修剪。
  pub fn prune(st: &mut Store, keep_runs: usize) -> bool
  ```

  返回"是否有删除"，实现直接复用 `crate::config::trim_runs(&mut st.state.runs, keep_runs)` 的判定逻辑（可以对 `trim_runs` 抽一个带 `removed_ids` 输出的内部函数，但**不得**出现第二份阈值/第二份"只删 terminal"的判断）。
- `trim_runs` 修剪时必须同时删除对应 run 的日志文件，且**只删** terminal（`RunStatus::terminal()`）的 run；`running`/`queued` 永久保留，即使超出阈值。
- 三个调用点改为传 `keep_runs`：`src/jobs.rs:207`、`src/schedules.rs:509`、`src/server.rs:2698`。
  调用点若拿不到配置，就给 `App` 增加字段 `pub keep_runs: usize`（`src/types.rs:381 pub struct App` 内），在 `src/main.rs` 构造 `App` 处从生效配置填入（`cfg.keep_runs`）。**不得**在调用点重新写死数字。
- 启动期 `src/main.rs:1226` 的 `trim_runs` 调用保持原行为（启动修剪 + save），但改为与运行期同一路径，避免两条实现。

## 二、单 run 日志封顶

- 新配置键 `max_log_bytes`：加入 `src/config.rs:16 CONFIG_KEYS`（12 → 13 键）、`Config`、`EffectiveConfig`、`EffectiveConfig::config_json`（`config.rs:92`）、`EffectiveConfig` 的表格取值分派（`config.rs:118` 附近）、`apply_set`（`config.rs:268`）、`apply_unset`（`config.rs:310`）与默认值解析（`config.rs:505` 附近），默认 **4194304**（4 MiB），`0` = 不限。
- 新增 `fn append_capped(path: &Path, chunk: &[u8], limit: usize) -> io::Result<()>`（放 `src/runner.rs`）：
  - `limit == 0` 时行为等同现在的 `OpenOptions::append`。
  - 写入前 `metadata().len()`；若 `len + chunk.len() > limit`，**只写入能装下的部分**，然后追加一行固定标记并停止后续写入：
    `\n[roamway: log truncated; max_log_bytes=<limit> reached]\n`
  - 标记只写一次：文件末尾已含该标记时直接返回而不写。实现上以"读文件最后 200 字节判断标记"为准，不要为此引入额外状态文件。
  - 已存在内容一律不重写、不截断（只停写），保证 SSE/尾部读取语义不乱。
- `src/runner.rs:149`、`:746` 两处追加写入改走 `append_capped`；`src/runner.rs:79` 的 `File::create`（新 run 首写）可保留，但同样要在超限时走封顶逻辑（首写就超限意味着 limit 极小，必须不 panic 且留下标记）。
- `append_capped` 需由 `App` 拿到 `max_log_bytes`（同 `keep_runs` 一样挂在 `App` 上），但纯函数签名不得依赖 `App`，以保可测。

## 三、`roamway prune` 子命令 + `POST /v1/prune`

- CLI：`roamway prune [--dry-run] [--keep N] [--yes]`
  - `--keep N`：本次临时覆盖 `keep_runs`，不改 config 文件；非数字/负数 → usage error exit 2。
  - `--dry-run`：只统计不删除，不改 state、不删日志。
  - `--yes` 保留为兼容位（无交互确认，接受但无副作用），未知参数一律 exit 2。
  - 输出（table）四列：`RUNS_REMOVED`、`LOGS_REMOVED`、`BYTES_FREED`、`MODE(dry-run|apply)`；`-o json` 输出同名字段（小写下划线）。
- 服务端：`src/server.rs:1270 fn route` 增 `POST /v1/prune`，body `{"dry_run": bool, "keep": number|null}`；返回 `{"ok":true,"runs_removed":N,"logs_removed":N,"bytes_freed":N,"dry_run":bool}`。
  - 鉴权沿用现有写操作要求（与 `POST /v1/runs` 同级别，走 `require_approval`/token 既有路径，不新造鉴权分支）。
  - `dry_run` 分支必须**不加锁写**：只读统计后返回。
- CLI 实现走现有 `dispatch_json` 风格（`src/main.rs:799`），远端与 `--data` 本地两种模式都要有（与其它子命令一致）。

## 四、`/metrics` 字节 gauge

在 `src/server.rs:466` 的 `/metrics` 文本里新增（Prometheus 文本格式，`# HELP`/`# TYPE` 齐全）：

```
roamway_state_bytes            # state.json 文件大小（不存在则 0）
roamway_logs_bytes             # <data>/logs 下 *.log 合计字节
roamway_logs_count             # <data>/logs 下 *.log 文件数
```

- 取数用 `std::fs::metadata`/`read_dir`，失败一律记 0，绝不 panic、不阻塞请求。
- 只统计 `<data>/logs/*.log`，不递归子目录（避免把 worktree 里的日志算进来）。
- 与现有 gauge 同一次状态锁内取值，不要新开长锁。

## 五、文档

- `docs/config.md`：新增 `max_log_bytes` 行（类型/默认 4 MiB/0=不限），并在 `keep_runs` 行补一句"运行期与启动期同一阈值，0=不修剪"。
- `docs/troubleshooting.md`：新增一节「state.json / logs 膨胀」，给出 `roamway prune --dry-run` → `roamway prune` 的步骤，并说明 `/metrics` 三个字节指标怎么读。
- `README.md`：Roadmap 追加一条已完成项 `- [x] 长跑保留策略与日志封顶（P22）`（不要动其它勾选项）。
- 不改 `Cargo.toml` 版本号，不写 CHANGELOG（版本与 CHANGELOG 只由发布阶段统一处理）。

## 六、测试（新增 ≥ 9 个，且必须覆盖负路径）

`src/config.rs`
1. `config_keys_include_max_log_bytes`
2. `max_log_bytes_defaults_to_4mib`
3. `max_log_bytes_rejects_zero_negative_and_garbage`（`set` 时负数字符串/garbage → Err；`0` → Ok 且表示不限）

`src/jobs.rs`
4. `prune_uses_keep_runs_not_hardcoded`（构造 900 条 terminal run，`keep_runs=100` → 剩 100；旧实现会留 800）
5. `prune_zero_keeps_everything`
6. `prune_never_drops_running_or_queued`（超出阈值的 running 保留）
7. `prune_removes_log_files_of_dropped_runs`

`src/runner.rs`
8. `append_capped_stops_at_limit_and_marks_once`（limit 小、连写三次 → 文件 ≤ limit+标记长度，标记只出现一次，且限于 UTF-8 边界不产生 panic）
9. `append_capped_zero_means_unlimited`

`src/server.rs`
10. `metrics_include_state_and_logs_bytes`
11. `prune_endpoint_dry_run_does_not_mutate_state`

`src/main.rs`
12. `prune_cli_rejects_bad_keep`

## 七、验收（外部在 NAS 上跑）

- `cargo build --release` 0 warning；`cargo test` 全绿且总数 ≥ 214（P21 后基线 212+）。
- `cargo fmt --check` 通过（若仓库已启用；不启用则跳过，不要新增 fmt 要求）。
- 手工冒烟（NAS 生产数据副本上）：
  1. `roamway prune --dry-run -o json` → 字段齐全、`dry_run=true`，`state.json` mtime 不变。
  2. `roamway prune --keep 5` → `runs_removed > 0`（若历史足够），`roamway ls --runs` 条数 ≤ 5 + 非终态条数。
  3. `curl -s -H "Authorization: Bearer $TOKEN" localhost:6768/metrics | grep -E 'roamway_(state|logs)_'` → 三个指标都有值，`logs_bytes` 与 `du -sb <data>/logs` 同数量级。
  4. 造一个大日志 run（`roamway run shell -- 'seq 1 200000'`，limit 设小）→ 日志停止增长并含 truncate 标记。
- 三平台 CI 探针必须绿（Windows job 会跑 `scripts/smoke.ps1`）。

## 八、硬边界

- 不新增 crate；不改 P1–P3 已验收的 CLI/HTTP 行为与退出码契约。
- `prune` 只删 terminal run 及其日志文件；**绝不**删 agent 记录、worktree 目录、schedules、jobs、approvals。
- 所有新文件/改动写成 UTF-8（含中文注释），严禁 GBK 输出。
- 不执行 `git push --force`，不动 `main` 之外的发布流程。
