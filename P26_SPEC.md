# P26 — 数据目录单写者 + store 健康可观测

## 背景与已勘事实（实现前请自行复核行号）

P25 已上线（prod `roamway 1.2.0`，`/healthz` `/readyz` 均 200，`/metrics` 401）。但环绕
"同一个 data 目录只该有一个写者"和"降级启动是否对外可见"仍有三个真洞：

1. **同一 data 目录可被两个 daemon 同写**：`cmd_serve`（`src/main.rs:1252`–`1258`）在
   `Store::load` 之后无条件 `backup::write_pid_file(&data)`；而 `write_pid_file`
   （`src/backup.rs:278`）是 tmp+rename 直接覆盖，**不检查现有 owner**。第二个
   `roamway serve --data <同一目录>`（换个 `--bind` 端口即可绕过端口冲突）会静默接管 pid 文件、
   与第一个实例双写 `state.json`；第一个实例退出时 `clear_pid_file`（`backup.rs:293`）因 owner
   不符而不删，于是留下指向死进程的陈旧 pid，`daemon status` / `restore` 的活体判定随之被骗。
2. **store 健康被压成一个 bool 且对外不可见**：`Store.load`（`src/types.rs:248`）的 `loaded_ok`
   把三种完全不同的起点混在一起——全新空目录、从 `.bak` 恢复（降级但可用）、`.bak` 也坏（历史真丢）。
   `readyz`（`src/server.rs:2541`）只看 `try_lock`，`healthz` 只看 uptime，于是**跑在空库上的
   daemon 对外照样报 200 `ok:true`**，编排器会把流量导给一个已经忘记自己历史的进程。
3. **降级启动会把最后的救命副本换掉**：坏文件隔离走 `state.json.corrupt-<epoch>`
   （`types.rs:366`–`372`），但 `let _ = fs::rename(...)` 吞掉失败；更糟的是隔离成功后
   `save()` 的第二次落盘会把当前 `state.json` 滚进 `.bak`，**覆盖掉唯一可用的恢复副本**——
   降级启动后一分钟内，`.bak` 里就只剩空库内容。

## 目标

让"一个 data 目录只有一个写者"成为硬不变量（第二个实例 exit 3，而不是双写），并让 store 的
降级状态能被 `/readyz`、`roamway daemon status` 看见；降级启动时保住可用的备份副本。
不新增 crate 依赖，不引 libc/ctrlc，不改 `/health`、`/v1/health` 的键集，不动 P1–P3 已验收行为。

## 一、serve 单写者锁

- 新增 `<data>/roamway.lock`（内容：`<pid> <bind> <iso>` 一行），用
  `OpenOptions::new().write(true).create_new(true)` 取得（`O_EXCL`，跨平台可用，不碰 flock）。
- `backup.rs` 新增：
  - `pub fn try_acquire_writer_with(data: &Path, bind: &str, self_pid: u32, alive: &dyn Fn(u32) -> bool)
    -> Result<u32, (u32, String)>`——核心实现，冲突时返回 `(owner_pid, owner_bind)`；
    `alive` 注入是为可测（单测不需要真起第二个进程）。
  - `pub fn acquire_writer(data: &Path, bind: &str) -> Result<u32, (u32, String)>`——
    薄封装，`alive` 传 `process_alive`，`self_pid` 传 `std::process::id()`。
  - `pub fn release_writer(data: &Path)`——仅当锁文件里的 pid 是当前进程才删（仿 `clear_pid_file`）。
- 规则（顺序固定）：
  1. `create_new` 成功 → 写入 pid/bind/iso，`sync_all`，返回 Ok。
  2. 已存在 → 解析 pid；`alive(pid) && pid != self` → Err((pid, bind_from_file))，**不覆盖**。
  3. 已存在但 pid 已死（`process_alive == false`）或 pid 就是 self（上次自己崩在启动早期）→
     `remove_file` 后**重试一次**（只重试一次；第二次仍失败就 Err）。
  4. 锁文件内容解析不出来（0 字节/乱码）→ 按 stale 处理（同上，删+重试一次）。
- `cmd_serve` 接线：
  - 取锁位置：`resolved_config` 之后、`Store::load` **之前**（先拿写者身份，再读盘）。
  - 冲突时不启动，stderr 一行：
    `roamway: data dir <data> is already served by pid <N> (bind <B>); stop it first or pass --force`，
    `std::process::exit(3)`（与 `restore` 的 exit 3 "有活体"语义一致）。
  - `--force`（`BOOLS` 已含 `"force"`）→ 跳过存活校验，直接覆盖锁文件；但必须
    `eprintln!("roamway: --force taking over data dir <data> from pid <N>; two writers will corrupt state.json")`。
  - `server::serve` 返回 Ok（正常退出）时调 `release_writer`；`Err` 分支（`serve failed`）在
    `exit(1)` **之前**也要 `release_writer`，否则下次启动被自己的残留锁挡住。
  - `roamway daemon stop` 走 kill，残留锁由 stale 路径自动接管——**不要**为此加信号处理。

## 二、Store 健康三态/四值

- `types.rs` 新增（不参与 serde 序列化）：
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum StoreHealth { Fresh, Ok, Recovered, Unrecoverable }
  ```
- 判定：
  - `Fresh`：`state.json` 与 `state.json.bak` **都不存在**（全新目录，正常）。
  - `Ok`：`state.json` 解析成功。
  - `Recovered`：`state.json` 存在但坏（解析失败）或读失败，且 `.bak` 可解析。
  - `Unrecoverable`：两者都不可用（`.bak` 也坏/不存在）。
- `Store` 新增 `pub store_health: StoreHealth`；**保留 `loaded_ok` 字段**并令
  `loaded_ok = store_health != StoreHealth::Unrecoverable`，避免 P1–P3 既有测试与 `cmd_serve`
  的 `if loaded_ok { app.save() }` 语义漂移；新逻辑一律读 `store_health`。
- `readyz`（`server.rs:2541`）：先 `try_lock`（忙仍是 503 `busy`，优先级与 P25 完全一致）；
  拿到锁后看 `store_health`：
  - `Fresh`/`Ok`/`Recovered` → 200，body 追加 `"store": "fresh"|"ok"|"recovered"`（原有
    `ok/version/uptime_s` 三个键全部保留）。
  - `Unrecoverable` → 503 `{"ok": false, "reason": "state_unreadable", "store": "unrecoverable"}`。
- `healthz`（liveness）**不改**：不读盘、不取锁（P25 已验收）。
- `/health`、`/v1/health` 键集**一个键都不许加**（硬约束，P1–P3）。

## 三、降级启动的数据保底

- 隔离改名失败时 `eprintln!` 报错，不再 `let _ =` 静默吞。
- 隔离成功后：若 `.bak` 可解析，把它复制成 `state.json.bak.known-good`（**已存在则不覆盖**，
  永远保护最早那份已知良好副本）。
- `Store` 新增 `preserve_backup: bool`（初值 = `store_health != Ok`）：`save()` 里把
  `if p.exists() { rename(p, bak) }` 改成 `if p.exists() && !self.preserve_backup { ... }`；
  首次成功落盘后置 `false`。这样降级启动后的第一次写入不会覆盖 `.bak`。
- 坏文件快照有界：新增 `pub fn prune_corrupt_snapshots(dir: &Path, keep: usize)`，
  按文件名里的 epoch 排序只留最近 `keep`（=5）个；`Store::load` 末尾调用。
- 版本与文档：`Cargo.toml` → `1.3.0`；`CHANGELOG.md` 新增 `## 1.3.0` 段（放在 `## 1.2.0`
  之上，`release.yml` 取第一个 `## ` 段），并**删掉 1.2.0 段 `Changed` 末尾那句错误残留**
  （"`roamway --version` 与 `/health` 的 version 仍为 `1.1.0`；统一升到 `1.2.0` 留给下一批"
  —— P25 已经升过了，这句现在是假话）；README Roadmap 勾 P26。

## 四、CLI 与文档可见性

- `roamway daemon status`（`main.rs:4229` → `cmd_daemon_status`）：
  - JSON 增加 `"store": "ok|fresh|recovered|unrecoverable|unknown"` 与 `"writer": {"pid":N,"bind":"..."}|null`；
  - 文本模式加一行 `store <state>  writer pid <N> bind <B>`（无锁时 `writer none`）；
  - **daemon 离线也要能报 store 状态**（直接读本地 `state.json` 体检，不依赖 HTTP）。
- `roamway daemon stop` 成功后，若发现残留锁文件，打印一行"stale writer lock will be taken over
  on next start"（不自动删，交给启动路径）。
- usage 面：`serve` 的头部/dispatch/`usage_error` 三处串都补 `[--force]`；`docs/cli.md` 签名行同步；
  `docs/troubleshooting.md` 增两节：「第二个实例为什么起不来（exit 3，如何确认谁在跑）」与
  「`/readyz` 回 503 `state_unreadable` 怎么处置（备份在哪、如何用 `restore` 回滚）」。

## 五、测试（≥14 条，全部纯单测，不依赖端口/NAS）

1. `acquire_writer_creates_lock_with_pid_and_bind`
2. `second_writer_refused_when_owner_alive`（注入 `alive=|_| true`，期望 `Err((pid, bind))`，锁文件内容不变）
3. `stale_lock_taken_over_when_owner_dead`（注入 `alive=|_| false`，期望 Ok 且内容更新为 self）
4. `stale_lock_taken_over_when_owner_is_self`
5. `garbage_lock_file_treated_as_stale`
6. `release_writer_only_when_owner`（别人写的锁不删）
7. `force_takeover_overwrites_live_lock`（serve 层语义：`--force` 路径不复用 `acquire_writer` 的存活校验）
8. `busy_message_mentions_pid_and_bind`
9. `store_health_fresh_on_empty_dir`
10. `store_health_ok_on_good_state`
11. `store_health_recovered_from_backup`
12. `store_health_unrecoverable_when_both_bad`
13. `readyz_503_when_state_unreadable`
14. `readyz_200_carries_store_field`
15. `save_skips_backup_rotation_while_preserving`
16. `corrupt_snapshots_capped_at_five`
17. `load_still_marks_stale_runs_interrupted`（回归，防 `Store::load` 重构破坏 P1–P3）

## 验收

- NAS 门禁：`cargo build --release` 零 warning，`cargo clippy -- -D warnings` 通过，
  `cargo test` 全绿（现有 257 条 + 本阶段新增 ≥14）。
- 生产冒烟（NAS，**不碰生产 data**，一律在 `/tmp/p26smoke` 副本上做）：
  - 副本上起第二个实例（不同 `--bind`，同 `--data`）→ **exit 3**，stderr 含 `already served by pid` 与 bind；
  - 加 `--force` → 能起来且打印 `--force taking over` 警告（随后杀掉，清理端口）；
  - 造坏 `state.json` + 好 `.bak` → 起 daemon → `/readyz` 200 且 `"store":"recovered"`，
    `state.json.bak.known-good` 存在；
  - 两者都坏 → `/readyz` 503 `state_unreadable`，`/healthz` 仍 200；
  - 生产实例不重启：`--version` = `1.3.0`（重启后），`/healthz` `/readyz` 仍 200，`/metrics` 仍 401。
- 三平台 CI 绿（`ci-probe-*` 探针分支，红则 main 不污染）。
- 不变量：`/health`、`/v1/health` JSON 键集前后完全一致；`roamway run/ls/status` 等 P4–P24
  已验收命令的行为与退出码不变。
- 硬边界：不加 crate 依赖；不引 libc/ctrlc/signal-hook；不为锁引入后台线程。
