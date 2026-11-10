# P23 — 无人值守纵深：按时间保留 + `backup`/`restore` + state 冗余写治理 + 数据目录指标

## 背景与已勘事实（实现前请自行复核行号）

P22 把"条数"维度的保留、日志封顶、`prune` 与字节 gauge 收口了，但长跑仍有四个洞：

1. **保留只有条数维度**：`src/config.rs:572 fn trim_runs_plan(runs: &[Run], keep_runs: usize)` 只按总数修剪；
   `Run.created_at: i64`（`src/types.rs:139`）已有时间戳却没用。低频写入的 daemon（例如一周一个 run、总数远低于
   `keep_runs=500`）会让 2023 年的 run 永久留在 state.json 里，`prune` 对此无能为力。
2. **没有备份/恢复入口**：`src/main.rs:1008 fn dispatch` 的命令表里没有 `backup`，也没有 `restore`。
   现有兜底只有 `Store::save` 轮换出的 `state.json.bak`（`src/types.rs:292`–`320`，写 tmp → `sync_all` →
   rename 旧文件为 `.bak` → rename tmp 为正式文件）与 `schedules.json.bak`。用户要"把状态带走/回滚"只能手工 `cp`。
3. **`Store::save` 无条件全量写盘**（`src/types.rs:292`）：每次调用都 `to_string_pretty` 整个 `State`，然后
   `File::create` + `sync_all` + 两次 rename。内容与上一次写入完全相同时也会重写（心跳、只读请求后的空保存等场景），
   在 NAS 上表现为无谓的 COW 写放大与文件 mtime 抖动；无法从指标上看出这类浪费。
4. **`/metrics` 只有 `state_bytes` / `logs_bytes` / `logs_count`**（`src/server.rs:457`–`477`）：缺少把整个
   `<data>` 目录（含 `schedules.json`、`config.json` 等）算进去的单一数字，也没有写盘次数计数。

## 目标

让"无人值守三个月"这件事在**时间维度**也有界、在**事故后**可恢复、在**写放大**上可控，并且这些都能从指标里看出来。
不新增 crate 依赖（保持零依赖策略），不引入后台线程/定时器。

## 一、时间维度保留：`keep_days`

- 新配置键 `keep_days` 加入 `src/config.rs:16 CONFIG_KEYS`（13 → 14 键）、`Config`、`EffectiveConfig`、
  `EffectiveConfig::config_json`、`EffectiveConfig` 的表格取值分派、`apply_set`、`apply_unset` 与默认值解析。
  类型 `u64`，默认 **0 = 不限**。`set` 接受非负整数；负数/非数字/garbage → `Err`（与 `max_log_bytes` 同风格）。
- 保留判定仍**只有一处**（不得出现第二份阈值或第二份"只删 terminal"判断）：

  ```rust
  /// keep_runs=0 表示不按条数修剪；keep_days=0 表示不按时间修剪。now 为 unix 秒。
  pub fn retention_plan(runs: &[Run], keep_runs: usize, keep_days: u64, now: i64) -> Vec<String>
  ```

  - 现行 `trim_runs_plan(runs, keep_runs)` 改为 `retention_plan(runs, keep_runs, 0, 0)` 的薄封装，
    保证既有测试与调用点零改动通过。
  - 语义：候选集 = 全部 `status.terminal()` 的 run（`running`/`queued` **永久保留**，即使超龄/超数）。
    1) 时间优先：`created_at < now - keep_days * 86400` 的候选全部进删除集（`keep_days > 0` 时）；
    2) 条数兜底：从剩余 run 里按"总数 > keep_runs"的超额量，按 `created_at` 升序补进删除集（`keep_runs > 0` 时）；
    3) 删除集按 id 去重，顺序确定（同一 `created_at` 时按 id 字典序），保证测试可断言。
  - `now` 由调用方传入（不在此函数里读时钟），便于测试。
- `App` 增加字段 `pub keep_days: u64`（`src/types.rs:381 pub struct App` 内，紧邻 `keep_runs`），
  在 `src/main.rs` 构造 `App` 处从生效配置填入。三个调用点（`src/jobs.rs:208`、`src/schedules.rs:509`、
  `src/server.rs:2749`）与 `jobs::prune` 一起改为传 `keep_runs` + `keep_days`：

  ```rust
  pub fn prune(st: &mut Store, keep_runs: usize, keep_days: u64) -> bool
  ```

  调用点重新写死数字一律不允许。启动期在 `src/main.rs` 修剪处同样走这条路径。
- `prune` 子命令（`src/main.rs:1763 fn cmd_prune`）新增 `--max-age-days N`：本次临时覆盖 `keep_days`，
  不改配置文件；非数字/负数 → usage error exit 2。HTTP `POST /v1/prune` body 支持可选 `"max_age_days": number|null`。
  响应 JSON **只增不改**：保留现有 `ok/runs_removed/logs_removed/bytes_freed/dry_run`，
  新增 `age_removed`（被时间规则删掉的条数，`dry_run` 时同样是"将要删"）。
  table 输出保持现有四列 `RUNS_REMOVED LOGS_REMOVED BYTES_FREED MODE` 不变（列变更会破坏既有脚本）。

## 二、`backup` / `restore` 子命令（仅本地 CLI）

零依赖：`sha256` 自己实现，放新文件 `src/backup.rs`（`pub fn sha256_hex(bytes: &[u8]) -> String`）。
必须通过 RFC 6234 已知向量测试（空串、`"abc"`、`"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"`）。

### `roamway backup [--out DIR] [--keep N]`

- `--out` 缺省为 `<data>/backups`。在 `--out` 下新建 `roamway-backup-<YYYYmmdd-HHMMSS>/`
  （时间取本地时区偏移 `tz_offset_hours`，与现有时间渲染一致；同秒重复时追加 `-2`、`-3` 后缀，不覆盖已有目录）。
- 目录内文件（存在才复制）：`state.json`、`config.json`、`schedules.json`，外加 `manifest.json`：

  ```json
  {"format":"roamway-backup/1","created_at":<unix>,"version":"<crate 版本>",
   "files":[{"name":"state.json","bytes":1234,"sha256":"..."}]}
  ```

- 复制前先 `Store::save()` 落盘（若 daemon 在跑则通过既有 `request()` 路径触发一次保存，拿不到就按现有文件复制，
  不阻塞）。**绝不**复制 `token`、`pairing.json`、`passwords*` 等凭据文件——备份只含上表三件套。
- `--keep N`：备份成功后，只保留按目录名排序最新的 N 个 `roamway-backup-*`，其余整目录删除；
  `0` = 不清理；`N` 非法 → exit 2。输出 table 三列 `BACKUP_DIR FILES BYTES`；`-o json` 输出
  `{"ok":true,"dir":"...","files":3,"bytes":N,"pruned":N}`。
- 目录不可写/复制失败 → exit 1，且不留下半成品目录（失败时删除已建的备份目录）。

### `roamway restore --from DIR [--force] [--yes]`

- `DIR` 必须含 `manifest.json` 且 `format == "roamway-backup/1"`；逐文件校验存在、`bytes` 与 `sha256` 匹配。
  任一不符 → exit 1，**一个字节都不写**，stderr 指明是哪个文件哪个字段。
- daemon 存活检测：`<data>/roamway.pid` 存在且该 pid 进程存活 → 无 `--force` 时 exit 3，
  提示 `restore: daemon looks alive (pid N); stop it or pass --force`。`--force` 只跳过停服检查，
  不做热替换（仍需写前把现有 `state.json` 轮换为 `state.json.pre-restore-<YYYYmmdd-HHMMSS>`）。
- 写入顺序：`config.json` → `schedules.json` → `state.json`（state 最后，保证半途失败时状态机的入口文件仍是旧的完整文件）。
  写每个文件用 tmp + `sync_all` + rename（与 `Store::save` 同一手法）。
- 输出：table 两列 `RESTORED FROM`，`-o json` 输出 `{"ok":true,"files":["state.json",...],"from":"..."}`。
- `backup`/`restore` **只做本地**：显式带 `--host` 时一律 exit 2 并提示
  `backup/restore are local-only`（`--data` 生效）。命令表 `src/main.rs:982` 的 usage 行与 `src/main.rs:1108`
  的分派手臂都要加，未知参数 exit 2。

## 三、state 冗余写治理

- `Store` 增加 `last_write_hash: u64`（`src/types.rs` 的 `Store` 定义内，默认 0）。
- `Store::save` 在序列化成功后计算 FNV-1a 64 哈希；若 `hash == last_write_hash` 且目标 `state.json` 存在，
  **跳过**写盘（不创建 tmp、不动 mtime），并让调用方能观察到"跳过"发生：
  - `Store` 增加 `pub writes_total: u64` 与 `pub writes_skipped: u64`（普通字段，不进 state.json 序列化）。
  - `save()` 返回 `bool`（`true` = 真写了）。现有调用点忽略返回值即可，但**不得**因此改变语义。
- `App` 增加 `pub state_writes: AtomicU64` 与 `pub state_writes_skipped: AtomicU64`；`App::save`（`src/types.rs:414`
  附近的包装）把 `Store` 的增量累加进去。
- `/metrics` 新增：
  ```
  roamway_state_writes_total
  roamway_state_writes_skipped_total
  roamway_data_bytes
  ```
  `roamway_data_bytes` = `<data>` 下**顶层**普通文件（`state.json`、`state.json.bak`、`config.json`、
  `schedules.json`、`schedules.json.bak`、`bind`、`heartbeat`、`roamway.pid` 等）字节合计 + `logs/*.log` 合计；
  不递归子目录、不跟随符号链接、失败记 0、绝不 panic。
- 关键约束：跳过写必须只在"序列化字节完全相同"时发生。任何内容变化（含新增 run、状态迁移、配置字段）都必须真写。
  不引入延时/合并/定时器（那需要后台线程，超出本阶段范围）。

## 四、文档

- `docs/config.md`：新增 `keep_days` 行（u64 / 默认 0 / 0=不限 / 与 `keep_runs` 同时生效，先按时间再按条数）；
  在 `keep_runs` 行补一句"与 `keep_days` 共同构成保留策略，两者都在 `config::retention_plan` 一处判定"。
- `docs/cli.md`：新增 `backup` 与 `restore` 两节（参数、退出码 0/1/2/3、local-only 说明、manifest 格式）。
- `docs/troubleshooting.md`：新增「状态损坏 / 回滚」一节：`roamway backup --keep 7` 定期备份，
  从 `roamway-backup-*` 目录 `restore`，以及 `state.json.bak` 与备份目录的区别（前者是最后一次写的上一版，后者是显式快照）。
- `README.md`：Roadmap 追加 `- [x] 按时间保留 + backup/restore（P23）`（不动其它勾选项）。
- 不改 `Cargo.toml` 版本号，不写 CHANGELOG（版本与 CHANGELOG 只由发布阶段统一处理）。

## 五、测试（新增 ≥ 12 个，且必须覆盖负路径）

`src/config.rs`
1. `config_keys_include_keep_days`
2. `keep_days_defaults_to_zero`
3. `keep_days_rejects_negative_and_garbage`（`set` 负数字符串/garbage → Err；`0` → Ok）
4. `retention_plan_age_drops_only_terminal`（超龄 running/queued 保留）
5. `retention_plan_age_and_count_union_no_duplicates`
6. `retention_plan_zero_zero_is_noop`
7. `trim_runs_plan_still_wraps_retention_plan`（旧签名行为不变）

`src/backup.rs`
8. `sha256_matches_rfc6234_vectors`
9. `backup_writes_manifest_with_matching_hashes`（临时目录里造 state/config/schedules → 校验 manifest 三文件 hash 与 bytes）
10. `backup_keep_prunes_old_dirs`（造 3 个目录、`--keep 1` → 只剩最新）
11. `restore_rejects_tampered_hash_and_writes_nothing`
12. `restore_refuses_alive_daemon_without_force`（pid 文件指向当前测试进程 → exit 3 语义，用返回错误码断言）

`src/types.rs`
13. `save_skips_write_when_state_unchanged`（连续两次 save：文件 mtime/内容不变，`writes_skipped == 1`）

`src/server.rs`
14. `metrics_include_data_bytes_and_write_counters`

`src/main.rs`
15. `backup_restore_reject_remote_host_and_bad_flags`（`--host` → exit 2；`backup --keep -1` → exit 2）

## 六、验收（外部在 NAS 上跑）

- `cargo build --release` 0 warning；`cargo test` 全绿且总数 ≥ 235（P22 后基线 223+12）。
- 手工冒烟（NAS，真实 data 目录副本）：
  1. `roamway daemon config set keep_days 30` → `get keep_days` 返回 30；`set -1` → 非零退出。
  2. `roamway prune --dry-run --max-age-days 0 -o json` → 字段含 `age_removed`，`state.json` mtime 不变。
  3. `roamway backup --keep 3 -o json` → 目录生成、manifest 三文件 hash 与 `sha256sum` 一致；
     连跑 4 次后 `ls -d <data>/backups/roamway-backup-*` 只剩 3 个。
  4. `roamway restore --from <最新备份>` 在 daemon 运行时 → exit 3；停 daemon 后 → exit 0，`roamway ls --runs` 条数一致。
  5. `curl -s -H "Authorization: Bearer ***" localhost:8796/metrics | grep -E 'roamway_(data_bytes|state_writes)'`
     → 三个指标都有值；`data_bytes` 与 `du -sb <data>`（排除子目录）同数量级。
- 三平台 CI 探针必须绿（Windows job 会跑 `scripts/smoke.ps1`）。

## 七、硬边界

- 不新增 crate；不改 P1–P3 已验收的 CLI/HTTP 行为与退出码契约（新增命令/新增可选字段除外）。
- `backup` 绝不打包凭据文件；`restore` 绝不删除备份源目录。
- `retention_plan` 是唯一保留判定入口；`running`/`queued` 永不因保留策略被删。
- 所有新文件/改动写成 UTF-8（含中文注释），严禁 GBK 输出。
- 不执行 `git push --force`，不动 `main` 之外的发布流程。
