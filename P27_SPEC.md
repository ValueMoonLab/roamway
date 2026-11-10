# P27 — clippy 门禁转正 + `daemon status` 说真话

## 背景与已勘事实（实现前请自行复核行号）

P26 已上线（prod `roamway 1.3.0`，`/healthz` 200、`/readyz` 200 `store=ok`、`/metrics` 401，
273 单测全绿，CI run 42898968386 三平台绿）。P26 的验收还留下四个真洞：

1. **clippy 门禁是假的**：`ci.yml:24` 是 `cargo clippy -- -D warnings` 但带
   `continue-on-error: true`（`ci.yml:25`），`cargo fmt --check` 同样带（`ci.yml:23`）。
   于是 63 个 clippy error 一直被 CI 放行；`ci.yml:24` 还漏了 `--all-targets`，
   测试目标里的 5 条 error 连跑都没跑。
2. **`daemon status --data <非默认目录>` 报别人的数**：`cmd_serve` 走
   `backup::write_pid_file`（`backup.rs:278`）只写 `<pid>\n` 一行；而
   `read_pid`（`main.rs:938`）把第二行当 bind，读不到就回落常量 `127.0.0.1:8796`
   （`main.rs:946`）。实测：子实例 pid 1054968 刚起 0 秒、`writer.bind=18802`，
   `daemon status --data <子目录> --json` 却报 `uptime_s: 886`、
   `effective_bind: 127.0.0.1:8796`——那是生产实例的数。
3. **`effective_bind` 不是"生效"的 bind**：`main.rs:4282` 取 `resolved_config` 的 bind，
   完全不看 writer lock。非默认目录的实例用 `--bind` 起在 18802 时，status 仍报配置里的值。
4. **`store` 在活体实例上撒谎**：`main.rs:4296` 无条件走离线 `inspect_store_health`。
   一个从 `.bak` 恢复的实例，`/readyz` 报 `store=recovered`，status 却报 `ok`。

NAS 实测（`cargo clippy --all-targets --message-format=short -- -D warnings`，
rustc 1.99.1 / clippy 0.1.99）：**63 errors = 58（bin）+ 5（test 目标独有）**，
分布见 §一。

## 目标

1. `cargo clippy --all-targets -- -D warnings` 零 error 零 warning，并把 CI 的 clippy / fmt
   改成硬门禁（删掉两处 `continue-on-error`）。
2. `daemon status` 的三处（`effective_bind` / `uptime_s`+`version` / `store`）必须来自
   **该 data 目录的那个实例**，而不是默认端口上的生产实例。

不新增 crate 依赖，不改 P1–P3 已验收行为（`/health`、`/v1/health` 键集不动；
`daemon status` 只允许**新增**键，不许改既有键的语义），测试只增不减。

## 一、clippy 清零（58 + 5）

逐条按 `--message-format=short` 的位置修，**不允许**用 `#![allow(clippy::all)]`、
`-A warnings` 或回归 `continue-on-error` 糊过去。

机械项（全部要改成 clippy 建议的写法）：

- `literal with an empty format string`（约 20 处，全在 `main.rs`：`1537:54 1648:62
  1718:66 2031:72 2242:59 2275:77 2693:39 2800:43 3078:59 3243:60 3328:89 3442:62
  3857:58 4847:47 4919:58 5020:66 5100:69`）——`format!("{}", "字面量")` 直接写字面量。
- `redundant reference in \`format!\`` 参数：`auth.rs:384:63`、`main.rs:4706:63`（去掉 `&`）。
- `redundant closure`：`server.rs:2825:46`、`server.rs:3138:42`（换 `String::new`）。
- `redundant guard`：`main.rs:2960:46`；`this \`if\` statement can be collapsed`：`server.rs:867:5`、`946:9`。
- `consider using \`sort_by_key\``：`agents.rs:158:5`、`auth.rs:551:9`、`server.rs:1641:9`、`1801:5`、`2047:13`。
- `chunks_exact` → `as_chunks::<64>().0`：`auth.rs:75:25`、`backup.rs:89:25`；
  同时 `auth.rs:32:14`、`backup.rs:91:18` 的 `for i in 0..` 索引循环改成迭代器（两处是同一段，一起改）。
- `manually reimplementing \`div_ceil\``：`auth.rs:102:18`；`is_multiple_of`：`auth.rs:129:8`。
- `manual check for common ascii range`：`auth.rs:176:16`；`stripping a prefix manually`：`auth.rs:230:15`。
- `std::io::Error::other(_)`：`hosts.rs:208:26`、`terminals.rs:189:22`、`workspaces.rs:102:26`、
  `plugins.rs:98:26`、`speech.rs:37:22`。
- `unnecessary map of the identity function`：`server.rs:1540:50`（删 `map_err`）。
- `unnecessary closure used with \`bool::then\``：`main.rs:2364:20`、`3169:20`。
- `unnecessary use of \`get(..).is_some()/is_none()\``：`main.rs:1057:14`、`1057:45`。
- `creates a reference which is immediately dereferenced`：`main.rs:1485:35`（`&pos` → `pos`）。
- `\`format!\` in \`format!\` args`：`main.rs:2950:15`（用命名参数或先拼好再套）。
- `equality checks against false`：`cron.rs:99:12`、`102:12`、`105:12`（`== false` → `!`）。
- `returning the result of a \`let\` binding from a block`：`cron.rs:119:9`。
- `casting to the same type (\`i64\` -> \`i64\`)`：`types.rs:39:15`。
- `creates an owned instance just for comparison`：`workspaces.rs:158:30`（`to_string_lossy()` 借用比较）。
- `field assignment outside of initializer for Default::default()`（测试代码）：
  `config.rs:742:9`、`750:9`、`783:9`（改结构体字面量 + `..Default::default()`）。
- `used \`assert_eq!\` with a literal bool`：`hosts.rs:429:9`（`assert!(x)` / `assert!(!x)`）。

行为敏感项（**只能逐位保等价，不许顺手改算法**）：

- `agents.rs:64:13 this operation has no effect`——哈希混合里 `(t.rotate_left(17) ^ n.wrapping_mul(K))`
  外面套了一层空操作（`| 0` / `& u64::MAX` 之类）。删掉空操作即可，**哈希结果必须不变**；
  改完要用现有 agent 身份相关测试证明字节一致。

判断项（三条，按此处理，`#[allow]` 全仓库总数不得超 3 且每条必须带一行理由注释）：

- `agents.rs:181:1`（12/7）、`agents.rs:238:1`（16/7）：内部注册式 API，参数与协议字段一一对应，
  加 `#[allow(clippy::too_many_arguments)]` + 理由，**不要**为了消警告去拆签名或引入 builder
  （会改动大量调用点，风险大于收益）。
- `terminals.rs:736:1 items after a test module`：把文件末尾的 `mod tests` 移到文件最后，
  或把 test mod 之后的 item 移到它前面。只挪位置，不改代码。

门禁（NAS）：`cargo build --release` 绿 + `cargo clippy --all-targets -- -D warnings` 0 条 +
`cargo test` 仍 **273 passed / 0 failed**（新测试只增不减）。

## 二、CI 门禁转正

- `.github/workflows/ci.yml`：
  - `cargo clippy -- -D warnings` → `cargo clippy --all-targets -- -D warnings`，**删掉 `continue-on-error: true`**。
  - 先跑 `cargo fmt` 把全仓库格式化，再把 `cargo fmt --check` 的 `continue-on-error: true` 删掉。
    fmt 与 clippy 的修复放**同一个提交**，允许大 diff。
- 三平台都要绿。Windows 分支（`#[cfg(windows)]`）在 NAS 上编不到，因此本阶段的真正验收
  是驱动器推的 `ci-probe-p27-*` 三平台 CI：**必须 3/3 绿**。若 Windows 特有 lint 冒出来，
  属本阶段范围，必须在同一阶段内补提交修掉（不许把 clippy 再改回 `continue-on-error`）。

## 三、`daemon status` 三处对齐

事实（复核用）：`cmd_daemon_start`（`main.rs:4542`）写的是 `<pid>\n<bind>\n` 两行，
所以 `read_pid` 只在"用 `daemon start` 起的实例"上拿得到 bind；`cmd_serve` 经
`write_pid_file` 只写 pid 一行，于是 bind 一律回落 `127.0.0.1:8796`。

**不要改 pid 文件格式**：`backup::pid_file_owner`（`backup.rs:272`）把整个文件内容当单个 u32 解析，
改成多行会连带打断 `restore` / `daemon stop` 的活体判定（P24 已验收不变量）。修的是调用方。

1. **单一真源取 bind**（`main.rs:4282`）：
   - 顺序：① writer lock（`backup::writer_lock_owner`）里的 pid 存活 → 用它的 bind；
     ② 否则 `resolved_config(flags).bind`；③ 再否则默认 `127.0.0.1:8796`。
   - 命中 ① 时 `effective_bind` 用 writer 的 bind，并新增
     `"bind_source": "writer" | "config" | "default"`（只加在 CLI status，不进任何 HTTP 响应）。
2. **活体校验**（`main.rs:4301`）：`read_pid` 拿到的 pid 必须
   `backup::process_alive(pid)`；
   - 若 writer lock 存在且 pid 不同 → pid 文件是陈旧的，`status["pid"]` 别信它，改用 writer 的 pid，
     并把旧值记进新键 `"stale_pid"`（数字或 null）。
   - pid 已死 → `ok: false`、`uptime_s: 0`、`error: "stale pid file (pid N not running)"`、
     `pid: null`（保留 `stale_pid: N`），**绝不再去连任何端口**。
3. **uptime/version 只从该实例取**（`main.rs:4303`–`4317`）：确认 pid 活体后，
   用 `effective_bind` 构造 `Client`（`hosts::endpoint_normalize`）打 `GET /health`；
   失败 → `ok: false`、`error: "healthz unreachable at <bind>"`、`uptime_s: 0`。
   不得再出现"bind 与实例无关却能拿到 200"的路径。
4. **store 取活体真值**：先 `GET /readyz`（带 `local_token`）：
   - 200 → `store` = body 的 `store`，`ready: true`，`store_source: "live"`；
   - 503 且 `reason == "state_unreadable"` → `store: "unrecoverable"`、`ready: false`、
     `ready_reason: "state_unreadable"`、`store_source: "live"`；
   - 503 且 `reason == "busy"` → `ready: false`、`ready_reason: "busy"`，`store` 保留离线体检值；
   - 401（token 不可用）→ `store` 保留离线体检值，`store_source: "offline"`，
     并在新键 `store_error` 记原因；
   - 完全无活体（无 writer / pid 死）→ 沿用 `inspect_store_health(&data)`，`store_source: "offline"`。
   `store` 的取值集合仍是 `fresh|ok|recovered|unrecoverable`（P26 已定），不许新增取值。
5. **文本输出**跟着改：`daemon online | version .. | pid .. | uptime ..s | data ..`、
   `config <file>  effective_bind <b> (<source>)`、`store <s>  <writer ..>  ready <yes|no>`。
   `--json` 里新键：`bind_source`、`stale_pid`、`ready`、`ready_reason`、`store_source`、`store_error`。

## 四、测试（只增不减）

把 status 的判定抽成可测纯函数（例如
`fn status_target(data: &Path, cfg_bind: &str, alive: &dyn Fn(u32) -> bool) -> (String, &'static str, Option<u32>, Option<u32>)`
返回 `(bind, bind_source, pid, stale_pid)`），注入 `alive` 便于测试，**不要**为测试真起进程。
新增至少 7 条：

1. writer lock 记 `18802`、pid 活 → `effective_bind=127.0.0.1:18802`、`bind_source=writer`。
2. 无 writer、config bind 为 8796 → `bind_source=config`。
3. pid 文件 pid 已死 → `ok=false`、`uptime_s=0`、`pid=null`、`stale_pid=Some(N)`、error 含 `stale pid file`。
4. pid 文件 pid 与 writer pid 不一致（两者都活）→ 以 writer 为准，`stale_pid` 记 pid 文件的旧值。
5. `/readyz` 200 `store=recovered` → `status.store=recovered`、`ready=true`、`store_source=live`。
6. `/readyz` 503 `state_unreadable` → `store=unrecoverable`、`ready=false`、`ready_reason=state_unreadable`。
7. 无 writer / 无 pid → `store_source=offline`，`store` 与 `inspect_store_health` 一致。

## 五、验收（NAS + CI + 生产）

1. NAS：`cargo build --release`、`cargo clippy --all-targets -- -D warnings`（0 条）、
   `cargo test`（≥273 全绿）。
2. CI：`ci-probe-p27-*` 三平台 3/3 绿（clippy/fmt 已是硬门禁）。
3. 生产冒烟（NAS，部署后）：
   - `/healthz` 200、`/readyz` 200 `store=ok`、`/metrics` 401；
   - 起第二个实例（`--data /tmp/p27x --bind 127.0.0.1:18812`）→
     `roamway daemon status --data /tmp/p27x --json` 的 `effective_bind` 必须是
     `127.0.0.1:18812`、`bind_source=writer`、`uptime_s` 是个位数、`pid` 等于该实例；
   - 生产目录的 `daemon status --json` 仍报生产的 uptime（不许被小实例串味）；
   - 清理第二个实例后 `daemon status --data /tmp/p27x --json` → `ok=false`、`pid=null`、
     `bind_source` 不为 `writer`。

## 六、不许碰的东西

- `/health`、`/v1/health`、`/healthz`、`/readyz` 的键集与状态码（P25/P26 已验收）。
- pid 文件格式与 `write_pid_file` / `clear_pid_file` / `pid_file_owner` 的语义。
- `daemon_alive` 的三条证据（P24 不变量）。
- writer lock 的获取/释放顺序与 exit 3 语义（P26 不变量）。
