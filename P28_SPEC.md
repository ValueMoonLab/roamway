# P28 — `roamway doctor` 运维自证 + HTTP 延迟/错误画像

## 背景与已勘事实（实现前请自行复核行号）

P27 已上线：prod `roamway 1.4.0`，NAS 门禁 `cargo build --release` / `cargo test`（282 passed）/
`cargo fmt --check` / `cargo clippy --all-targets -D warnings` 全绿，三平台 CI 已转硬门禁。
P27 的冒烟还暴露两类**运维缺口**，都不涉及协议、但直接决定"出事时能不能一句话说清"：

1. **没有"一次答完"的体检入口**。今天要判断"这台机器上的 daemon 现在能不能干活"，得分别跑
   `daemon status`、`health`、`ls`、手查数据目录权限、手查 `state.json`、比对版本——五六个命令，
   每个各自 exit 0 就算"成功"，谁都没有把结论汇总。更糟的是 `daemon status` 只回答
   "进程活着 + store 健康"，**不检查数据目录是否可写**：数据目录被 `chmod 500` 或被磁盘写满的实例，
   `daemon status --json` 仍是 `ok=true`（它只读文件、从不写），而真正要落盘时全 500。
2. **`/metrics` 只数请求、不量时间**。`src/server.rs:655/662` 已有 `roamway_http_requests_total` 与
   `roamway_http_requests_failed_total`（`>=400` 合并计数，App 字段 `req_total`/`req_failed`，
   `src/server.rs:3898–3900`），但没有延迟，也没有 4xx/5xx 分档：无法回答"慢在哪"，
   也无法区分"客户端拿错 token（401 洪峰）"与"服务端真在 500"。

本阶段只做这两件事。**零新 crate**（`libc` 已是 unix 依赖，用它足矣）。

## 目标

1. 新顶层命令 `roamway doctor [--json] [--data DIR] [--strict] [--timeout N]`：单次执行 8 项检查，
   人读 + JSON 双形态，退出码 `0` / `3` / `4` 三态。
2. `/metrics` 增请求延迟（sum/count/max）与 4xx/5xx 分档计数，纯内存、重启归零，既有指标名与类型不动。

不改 P1–P3 已验收行为（`/health`、`/v1/health` 键集；`serve/run/ls` 的 CLI 契约），
`daemon status` 的键集与语义不动（P27 不变量），测试只增不减。

## 一、`roamway doctor`

### 1.1 检查项（固定 8 条，id 稳定，输出顺序固定）

| id | 检查 | 判据 | PASS / WARN / FAIL / SKIP | 失败提示（hint） |
|---|---|---|---|---|
| `config` | 配置文件可解析 | `<data>/config.json` 不存在 → PASS（默认配置）；存在但 JSON 坏或键非法 → FAIL | `resolved_config` 复用现有解析 | 删除或修复 `config.json` |
| `data_dir` | 数据目录可写 | 目录不存在 → FAIL；写 `<data>/.doctor-probe` 成功并删除 → PASS | 命名探针文件，**不碰任何既有文件** | `chmod u+w <data>` / 检查磁盘满 |
| `writer` | 单写者状态 | 无 `roamway.lock` → PASS（daemon 未运行）；锁 pid 存活 → PASS 并报 pid/bind；锁 pid 已死 → WARN | 复用 `backup::writer_lock_owner` | 下一个 `serve` 可自动接管；如需立刻清理 `rm <data>/roamway.lock` |
| `pid_file` | pid 文件真实性 | 无 `roamway.pid` → PASS；pid 存活 → PASS；pid 已死 → WARN | 复用 `backup::pid_file_owner` + `process_alive` | `roamway daemon stop` 或手工删 `roamway.pid` |
| `store` | 状态存储完整性 | `types::inspect_store_health`：`fresh`/`ok` → PASS；`recovered` → WARN；`unrecoverable` → FAIL | `src/types.rs:458` | 从 `<data>/state.json.bak` 或备份恢复 |
| `bind` | 监听地址可解析 | 复用 `status_target` 得 `effective_bind`；`host:port` 可解析 → PASS（报告 `bind_source`）；不可解析 → FAIL | `src/main.rs:4692` | 修 `config.json` 的 `bind` 或 `--bind` |
| `live` | 活体探活 | 无存活 pid → SKIP；存活 → 用 `status_probe_base`（`src/main.rs:4686`）打 `GET /healthz`：200 → PASS 报 `version`/`uptime_s`；非 200 或连不上 → FAIL | 免鉴权，不带 token | 看 `roamway logs` / systemd `journalctl` |
| `version` | 版本漂移 | 无活体 → SKIP；二进制 `env!("CARGO_PKG_VERSION")` 与活体 `/health` 的 `version` 一致 → PASS；不一致 → FAIL | — | 重新部署后 `daemon restart`；旧进程不会自我升级 |

硬要求：

- `doctor` **只读**：不得写 `<data>` 下除 `.doctor-probe` 以外的任何文件、不得抢 writer lock、
  不得触发 run/schedule、不得改 `state.json`。探针文件无论成功失败都必须删除（失败也尽力删）。
- 检查项抽成纯函数，便于注入：`fn run_checks(data: &Path, cfg_bind: &str, alive: &dyn Fn(u32) -> bool, probe: &dyn Fn(&str) -> Option<LiveInfo>) -> Vec<Check>`。
  真实现里 `probe` 用 `Client { base: status_probe_base(bind), .. }` 打 `/healthz`；测试里用闭包伪造。
- 每条检查结构：`{ id, status, detail, hint }`，`status ∈ pass|warn|fail|skip`，`detail` 为一行人类可读事实。
- 汇总：`{ pass, warn, fail, skip }`，且 `pass + warn + fail + skip == 8`（测试要断言）。

### 1.2 输出形态

文本（每行一条，末行汇总）：

```
roamway doctor 1.4.0    data <data>    binary <path>
PASS config      config.json ok
PASS data_dir    writable
WARN pid_file    stale pid file (pid 1234 not running)
PASS store       ok (state.json 37 KB, backup 3)
...
4 pass, 1 warn, 0 fail, 3 skip
```

JSON（`--json`）：

```json
{
  "version": "1.4.0",
  "data": "<dir>",
  "ok": false,
  "checks": [{"id": "data_dir", "status": "fail", "detail": "...", "hint": "..."}],
  "summary": {"pass": 4, "warn": 1, "fail": 1, "skip": 2}
}
```

`ok` = `summary.fail == 0`（warn 不算失败）。

### 1.3 退出码

- `0`：`fail == 0`（允许 warn / skip）。
- `3`：`fail > 0`；**或** 加了 `--strict` 且 `warn > 0`（把"陈旧但能跑"也当红灯，给 cron 报警用）。
- `4`：用法/内部错误——未知 flag、`--data` 指向不可访问路径、`--timeout` 非数字/为 0。
  文案走 `usage_error` 风格，不打印半截报告。

`--timeout N`（默认 5，秒）只作用于 `live` 的 HTTP 探活，避免 doctor 在宕机端口上挂住。

## 二、`/metrics` 增量（只加不改）

App 侧新增三个计数器（内存态，`AtomicU64`，重启归零，与 `req_total` 同生命周期）：

- `req_us`：所有请求累计耗时（微秒）
- `req_max_us`：单请求最大耗时（微秒，CAS 取大）
- `req_4xx` / `req_5xx`：按响应码分档

计量点：请求进入处取 `Instant::now()`（与 `src/server.rs:675` 的 `req_total.fetch_add` 同一函数），
在写响应头/结束时按最终状态码累加。**不得**在锁内做浮点运算，`/metrics` 只做整数相除到 6 位小数。

新增输出（顺序放在既有三个 http 指标之后，既有指标一字不改）：

```
# HELP roamway_http_request_duration_seconds_sum Total time spent handling HTTP requests, in seconds.
# TYPE roamway_http_request_duration_seconds_sum counter
roamway_http_request_duration_seconds_sum 12.345678
# HELP roamway_http_request_duration_seconds_count HTTP requests timed since start.
# TYPE roamway_http_request_duration_seconds_count counter
roamway_http_request_duration_seconds_count 41
# HELP roamway_http_request_duration_seconds_max Slowest single HTTP request since start.
# TYPE roamway_http_request_duration_seconds_max gauge
roamway_http_request_duration_seconds_max 0.481516
# HELP roamway_http_requests_4xx_total HTTP responses with 4xx status since start.
# TYPE roamway_http_requests_4xx_total counter
roamway_http_requests_4xx_total 3
# HELP roamway_http_requests_5xx_total HTTP responses with 5xx status since start.
# TYPE roamway_http_requests_5xx_total counter
roamway_http_requests_5xx_total 0
```

约束：`requests_4xx_total + requests_5xx_total == requests_failed_total`（`>=400` 只可能是 4xx/5xx），
这条恒等式要有测试；不做直方图桶（手写桶是伪精度，且会撑大 `/metrics`）。

## 三、测试（只增不减，从 282 起）

doctor（纯函数注入，不真起进程、不联网）：

1. `config.json` 内容为 `{` → `config` FAIL，`fail>=1`。
2. `data_dir` 不可写（unix：`chmod 0o500`，用 `#[cfg(unix)]`；windows 分支断言探针写到只读目录时 FAIL 的逻辑走同一分支或 `#[cfg(not(unix))]` 跳过）→ FAIL，且探针文件未残留。
3. 干净目录（无 lock/pid/state）→ `store=fresh`、`live=skip`、`version=skip`、`pass>=4`、`fail==0`。
4. `roamway.pid` 写不存在 pid → `pid_file=warn`，退出码仍 0（不带 `--strict`）。
5. `state.json` 写坏 → `store=unrecoverable` → FAIL（`cfg` 允许时直接构造坏文件）。
6. `alive` 返回真但 `probe` 返回 `version=0.0.1` → `version` FAIL。
7. `probe` 返回 `None`（连不上）→ `live` FAIL。
8. `--strict` 下仅 warn → 退出码 3；不带 `--strict` → 0。
9. `summary` 四项之和恒为 8；`ok == (fail == 0)`。
10. `--json` 输出可被 `serde_json::from_str` 解析且键集含 `version/data/ok/checks/summary`。

metrics：

11. 计时器起止：一次请求后 `count>=1`、`sum>0`、`max>0` 且 `max <= sum`。
12. 分档恒等式：发 1 次带错误 token 的请求 → `4xx+1`、`failed_total` 同增；`4xx+5xx == failed_total`。
13. `/metrics` 文本里新增 5 个指标名存在，且既有 `roamway_http_requests_total` 等名字/类型未变。

## 四、验收

1. NAS 门禁：`cargo build --release` 绿、`cargo test` **≥ 295 passed / 0 failed**（282 + 新增）、
   `cargo fmt --check` 0、`cargo clippy --all-targets -D warnings` 0 条。
2. 三平台 CI 探针绿（`ci-probe-p28-*`）。
3. 生产冒烟（NAS，部署后）：
   - `roamway doctor --json` 退出码 0，`summary` 四项和 8，`live`/`version` 为 pass；
   - 在临时数据目录造一个陈旧 pid 文件 → `roamway doctor --data <tmp> --json` 退出码 0 且 `pid_file=warn`；
     加 `--strict` → 退出码 3（同一份数据，只是策略不同）；
   - `roamway doctor --data /nonexistent-readonly --timeout 1` → 退出码 4（用法错误路径）；
   - `/metrics` 连抓两次：`roamway_http_request_duration_seconds_count` 与 `_sum` 单调增，
     误 token 请求后 `roamway_http_requests_4xx_total` 递增，且 `4xx+5xx == failed_total`。

## 五、不许碰的东西

- `/health`、`/v1/health`、`/healthz`、`/readyz` 的键集与状态码（P25/P26 不变量）。
- `daemon status` 的既有键与 `store` 取值集合（P27 不变量）；`status_probe_base` 仍是唯一探活路径。
- 既有 `/metrics` 指标名、类型与 HELP 文本（对外契约，P4 起就在）。
- pid 文件格式、writer lock 格式与 exit 3 语义（P24/P26 不变量）。
- 不得新增 crate；`#[allow]` 全仓库仍限 3 条；不得回归 CI `continue-on-error`。
- doctor 不得写状态、不得抢锁、不得触发任务。
