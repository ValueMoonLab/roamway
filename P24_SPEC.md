# P24 — 运维自证：写计数单一真源 + pid 文件生命周期 + restore 活体判定加固

## 背景与已勘事实（2026-11-10 23:0x 在 NAS 生产实测，实现前请自行复核行号）

P23 上线后（commit `4a911b8`，238 passed）在生产上做冒烟，暴露两个"指标/保护说谎"的洞：

1. **`roamway_state_writes_total` 长期停在 1。** 生产 daemon 22:40 启动，23:02 跑
   `roamway run --agent shell "echo p23smoke"` 后 `<data>/state.json` mtime 由 1794321631 变为 1794322953
   （确实落盘），但 `/metrics` 仍是 `roamway_state_writes_total 1` / `roamway_state_writes_skipped_total 0`。
   根因：计数只在 `src/types.rs:448 fn App::save` 里按差值累加，而 `src/server.rs` 的请求路径
   （`1451`/`1610`/`1736`/`2811`/`2894`/`2907`/`2942`/`2970`/`3114`/`3128`/`3426` 等处）直接调
   `st.save()`，绕过累加器。→ 指标对绝大多数真实写入视而不见，P23 §三"调用方能观察到跳过发生"在生产上不成立。

2. **`<data>/roamway.pid` 根本没有维护者。** `src/main.rs:1193 fn cmd_serve` 只起了一个每 5 秒写
   `<data>/heartbeat` 的线程（`~1286`），从不写 pid。生产现场：pid 文件内容 `67751`（mtime 停在 11-09），
   实际 serve 进程 `732495`。而 `src/backup.rs:267/277/295 fn daemon_alive` 只做 `read_pid` +
   `/proc/<pid>` 存在性检查，完全依赖这份一天前的残留。

3. **因此 `restore` 的停服保护可被绕过（已实测）。** 在真实 data 目录副本上把 pid 文件写成死 pid `999999`，
   daemon 仍在 `0.0.0.0:8796` 服务、`heartbeat` 5 秒前才更新，`roamway restore --from <备份> --data <副本>`
   仍然返回成功并覆盖了 `state.json`。即 P23 验收项"daemon 活着时 restore 返回 exit 3"在生产事实上不成立
   —— 只有 pid 文件恰好正确时才成立（副本里写活 pid `732495` 时确实打印
   `restore: daemon looks alive (pid 732495); stop it or pass --force`）。

4. 现成的两个旁证没用上：`<data>/bind`（内容 `127.0.0.1:8796` 一类地址）与 `<data>/heartbeat`
   （每 5 秒写一次 ISO 时间戳，实测 age 5s）。

## 目标

让"运维自证"闭环：**计数器不依赖调用点自律，pid 文件由 serve 自己维护，restore 的"活体"判定不靠单一易腐文件。**
不新增 crate 依赖，不引入后台线程以外的新机制（heartbeat 线程已存在，可复用其节奏）。

## 一、写计数单一真源

- 删除 `src/types.rs:430/432` 的 `App::state_writes` / `App::state_writes_skipped` 两个 `AtomicU64`，
  以及 `App::save`（`~448`）里的差值累加；`App::save` 若保留，只作为 `self.store.lock()….save()` 的薄封装。
- `/metrics`（`src/server.rs:573`–`585`）改为在**已经持有 store 锁的那一段**（`495`–`518` 的 `{ … }` 块内）
  读取并保存 `st.writes_total` / `st.writes_skipped` 到局部变量，输出时用这两个局部变量；不得再读任何 `AtomicU64`。
- `Store::load`（`src/types.rs`）显式把 `writes_total` / `writes_skipped` 置 0，并加注释：
  仅内存态、不序列化进 `state.json`、语义是 "since daemon start"（`Store` 里已有 `last_write_hash: u64` 同一风格）。
- 同步清理所有构造点：`src/server.rs:3647`、`src/server.rs:3745`、`src/main.rs:1263`、`src/main.rs:5190`
  的 `state_writes:` 字段；`-D warnings` 下不得留死字段/死赋值。
- HELP 文案改成不再骗人的措辞：
  - `roamway_state_writes_total` → `state.json writes that reached disk since daemon start`
  - `roamway_state_writes_skipped_total` → `state.json writes skipped because serialized bytes were identical since daemon start`
- 语义不变：`skipped` 只在"序列化字节完全相同"时 +1（`Store::save` 现有判定不动）。

## 二、pid 文件生命周期（由 serve 自己维护）

- 在 `src/backup.rs` 已持有 `fn read_pid`（`~261`）的基础上，补三个小函数（与 restore 共用，避免第二份实现）：

  ```rust
  pub fn write_pid_file(data: &Path) -> std::io::Result<u32>; // tmp+rename，写 "<pid>\n"
  pub fn pid_file_owner(data: &Path) -> Option<u32>;          // 现 read_pid 的 pub 版
  pub fn clear_pid_file(data: &Path);                          // 内容仍等于本进程 pid 才删
  ```

- `cmd_serve`（`src/main.rs:1193`）在 store 加载、启动期修剪之后、打印 banner 之前调用 `write_pid_file(&data)`；
  失败只 `eprintln!` 警告，不阻断启动（只读环境/权限问题不应让 daemon 起不来）。
- serve 正常返回路径（accept 循环退出、bind 失败前的早退不涉及）调用 `clear_pid_file(&data)`。
- 信号语义（零依赖约束下必须写清楚，禁止为此引入 `ctrlc`/`libc`/`signal-hook`）：
  Rust 标准库没有信号 API，`SIGTERM` 杀进程时 pid 文件会残留 —— 这不作处理，交给 §三 的 heartbeat/bind 兜底判定；
  规范只要求"自己正常退出时清理、且只清理自己写的"。
- Windows 同样执行（`clear_pid_file` 走 `std::fs::remove_file`，无 unix 专用调用）。

## 三、restore 活体判定加固

- `daemon_alive` 扩成带证据的探测（保留 `daemon_alive` 作为兼容薄封装，或改名并同步所有调用点，
  但**只有一处**判定实现）：

  ```rust
  pub enum Alive { Pid(u32), Heartbeat(u64), Bind(String) }
  pub fn daemon_probe(data: &Path) -> Option<Alive>;
  ```

  判定顺序（短路，先命中先返回）：

  1. `pid_file_owner` 得到 pid 且该进程存活（linux：`/proc/<pid>` 存在；其它 unix：`ps -p`；windows：`tasklist` 或 `OpenProcess` 等价实现，**不得 panic**）→ `Pid(pid)`
  2. `<data>/heartbeat` 的 mtime 距现在 ≤ 60 秒 → `Heartbeat(age_secs)`
  3. `<data>/bind` 内容解析成 `host:port` 后可 `TcpStream::connect_timeout`（**150ms**）连上 →
     `Bind(addr)`；`0.0.0.0` / `::` / 空 host 一律按 `127.0.0.1` 处理。连接失败、文件缺失、
     格式非法一律当"无证据"，不报错、不 panic。
  4. 全部不满足 → `None`
- `restore`（`src/backup.rs:311`，现错误文案在 `~347`）无 `--force` 时：任一证据 → `Err`（exit 3），
  stderr 消息按证据类型区分：

  ```
  restore: daemon looks alive (pid 732495); stop it or pass --force
  restore: daemon looks alive (heartbeat 3s ago); stop it or pass --force
  restore: daemon looks alive (port 8796 accepts connections); stop it or pass --force
  ```

- `--force` 语义完全不变（只跳过停服检查，仍做 `state.json.pre-restore-<ts>` 轮换，写序 `config → schedules → state` 不动）。
- 判定必须在"校验 manifest 之后、写任何字节之前"执行（保持现有顺序，tampered 备份仍然先报校验错）。

## 四、文档

- `docs/troubleshooting.md`「状态损坏 / 回滚」节补：daemon 活着时 `restore` 会报三种证据之一；
  `--force` 在活体上强恢复会丢掉 daemon 内存里尚未落盘的运行记录（下次 save 覆盖回去），给出"先 `kill` 再 restore"的顺序。
- `docs/cli.md` `restore` 节补同样的三条证据与 `--force` 说明。
- `docs/config.md` 不变（无新配置键）。README Roadmap 追加 `- [x] 写计数单一真源 + pid/活体判定（P24）`，不动其它勾选项。

## 五、测试（新增 ≥ 10，覆盖负路径）

`src/types.rs`
1. `store_counters_reset_on_load`（先 save 一次产生计数，再 `Store::load` 同一目录 → 两个计数都是 0）

`src/server.rs`
2. `metrics_write_counters_come_from_store`（直接持锁 `st.save()` 两次：第一次真写、第二次内容相同被跳过 →
   metrics 文本里 `state_writes_total 1` 且 `state_writes_skipped_total 1`）。这是 P23 漏洞的回归测试，
   必须**不经过** `App::save` 就能看到计数。

`src/backup.rs`
3. `pid_file_roundtrip_and_owner`（写 → `pid_file_owner` == 本进程 pid）
4. `clear_pid_file_only_when_owner`（文件里是别的 pid → 不删；是本进程 pid → 删）
5. `daemon_probe_prefers_live_pid`（pid 文件写本进程 pid，heartbeat/bind 不存在 → `Pid`）
6. `daemon_probe_falls_back_to_fresh_heartbeat`（pid 文件死 pid + 新鲜 heartbeat → `Heartbeat`）
7. `daemon_probe_falls_back_to_open_bind_port`（pid 死、heartbeat 陈旧、`TcpListener` 绑 `127.0.0.1:0` 并把实际端口写进 `bind` → `Bind`）
8. `daemon_probe_ignores_stale_pid_and_old_heartbeat`（三样都不满足 → `None`）
9. `restore_refuses_when_only_heartbeat_is_fresh`（备份合法、pid 死、heartbeat 新 → restore Err，且 `state.json` 字节未变）

`src/main.rs`
10. `serve_pid_file_written_and_cleared`（把写/清逻辑抽成可测路径：调用 `backup::write_pid_file`+`clear_pid_file`
    后目录里 pid 文件出现又消失；这是 `cmd_serve` 的直接依赖，不启真实 serve）
11. `backup_restore_reject_remote_host_and_bad_flags` 保持通过（回归，不改）

## 六、验收（外部在 NAS 上跑）

- `cargo build --release` 0 warning；`cargo test` 全绿且总数 ≥ 248（P23 基线 238 + 10）。
- 手工冒烟（NAS）：
  1. 重启生产 daemon（`/home/GZX/roamway/start.sh` 前先 `pkill -f "roamway serve"` 再起）后，
     `<data>/roamway.pid` 内容 == `pgrep -f "bin/roamway serve"`；`kill -TERM <pid>` 后 pid 文件
     **允许残留**（§二：零依赖不做信号处理，SIGTERM 下必然留下），但残留不得再被信任：
     `process_alive` 判死，且 heartbeat 超 60 秒 / bind 端口关闭后 `restore` 放行（不要求 `--force`）。
     验收看「残留但不再被信任」，不要求「文件消失」。
  2. 生产 `/metrics`：一次 `roamway run --agent shell "echo p24"` 后 `roamway_state_writes_total` 严格 +≥1
     （P23 实测停在 1 不动 —— 这是本阶段的核心回归点）。
  3. 真实 data 目录副本 + 一个合法备份：pid 文件写死 pid、`heartbeat` 新鲜、`bind` 端口可连 →
     `restore` 退出码 3，提示含 "heartbeat" 或 "port"；`state.json` 字节数与 mtime 都不变。
  4. 同一副本：pid 死、heartbeat 改成 10 分钟前（`touch -d`）、`bind` 指向未监听端口 →
     `restore` 退出码 0，`roamway ls --runs --limit 100` 条数与备份里 state 的 runs 数一致。
  5. 三平台 CI 绿（Windows job 跑 `scripts/smoke.ps1`）。

## 七、硬边界

- **零新依赖**：禁止 `ctrlc` / `libc` / `signal-hook` / `socket2` / `sysinfo`。TCP 探测只用
  `std::net::TcpStream::connect_timeout`；进程存活判定只用 `/proc`、`ps -p`、`tasklist` 之类既有手段。
- 不改 P1–P3 已验收的 CLI/HTTP 行为与退出码契约（`restore` 仍然是 0/1/2/3，`--force` 行为不变）。
- 不删除任何既有指标名；只允许改 HELP 文案与计数来源。
- pid 文件只含数字与换行，绝不写 token/路径以外的敏感信息。
- 所有新增/改动文件写 UTF-8（含中文注释），严禁 GBK 输出；不执行 `git push --force`。
