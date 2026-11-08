# P2 规范：主机联邦与守护进程（PARITY P2，13 条命令）

目标：roamway 支持多主机注册、健康探测、跨机派活与本地守护进程管理。完成后 `paseo host ls` 能做的事，`roamway host ls` 必须能做。

依赖：仅 std + serde + serde_json + ureq（已有）。**不要引入 tokio/axum/reqwest/clap**。

## 1. `src/hosts.rs`（新增）

```rust
pub struct Host {
    pub id: String,          // host_ + 8 hex
    pub name: String,        // 唯一，[a-z0-9-]{1,32}
    pub endpoint: String,    // 归一化为 http://host:port，去掉尾部 /
    pub token: String,       // 明文存盘但文件 0600；输出一律 mask
    pub enabled: bool,
    pub created_at: i64,
    pub last_probe: Option<Probe>,
    pub failures: u32,       // 连续失败次数
    pub next_retry_at: Option<i64>,
}
pub struct Probe { pub at: i64, pub ok: bool, pub latency_ms: u64, pub version: String, pub error: String }

pub struct Registry { pub path: PathBuf, pub hosts: Vec<Host> }
impl Registry {
    pub fn load(dir: &Path) -> Self;      // 不存在则空；损坏则备份为 hosts.json.corrupt-<ts> 后空启动
    pub fn save(&self) -> io::Result<()>; // tmp + fsync + rename；Unix chmod 0600
    pub fn add(&mut self, name, endpoint, token) -> Result<&Host, String>;
    pub fn remove(&mut self, name: &str) -> bool;
    pub fn get(&self, name_or_endpoint: &str) -> Option<&Host>;  // 支持按 name 或 http://ip:port 命中
}
```

- `endpoint` 归一化：`10.0.0.91:8796` / `tcp://10.0.0.91:8796` / `http://10.0.0.91:8796/` 都接受，存 `http://10.0.0.91:8796`。
- 退避：`next_retry_at = now + min(30 * 2^(failures-1), 3600)`；探测成功清零。
- `mask(token)` → `abcd…wxyz`（长度 <8 时 `****`）。

## 2. 探测与跨机派活（`src/hosts.rs` 内）

```rust
pub fn probe(h: &Host) -> Probe;                                  // GET {endpoint}/health, 5s 超时
pub fn dispatch(h: &Host, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String>;
```

- `dispatch` 一律带 `x-roamway-token`，超时 60s；网络错误返回 `Err(msg)`。
- 探测遵守 `next_retry_at`：未到时间直接返回上次结果并标 `skipped`。

## 3. CLI（`src/main.rs`）

| 命令 | 行为 |
|---|---|
| `host add <name> --endpoint <ep> [--token <t>]` | 缺 token 时读 `ROAMWAY_TOKEN_<NAME 大写、- 变 _>`；已存在同名则报错，`--force` 覆盖 |
| `host ls [--probe] [--json]` | 表格：NAME ENDPOINT STATE VERSION LATENCY RETRY；`--probe` 主动探测，默认用缓存 |
| `host rm <name>` / `host enable <name>` / `host disable <name>` | 改注册表 |
| `host probe <name>` | 单机探测，返回 JSON |

全局 `--host <name|endpoint>`（也可用环境变量 `ROAMWAY_HOST`）：作用于 `run/ls/logs/stop/inspect/status`。

- `run --host nas ...`：`dispatch` 到远端 `POST /v1/runs`，原样回显远端 `{"run":{...}}` 并在末尾追加 `"host":"nas"`。
- `ls --host nas`：取远端 `GET /v1/runs`，每条加 `"host":"nas"`。
- `ls --host all`：并发探测所有 enabled 主机，合并结果，按 `created_at` 倒序；单机失败不影响其他，失败信息进表尾 `errors`。
- `logs/stop --host`：对应远端路径。
- 远端未注册的 `--host` 值：当成裸 endpoint 直接请求（token 取 `ROAMWAY_TOKEN`），失败明确报错。

## 4. `daemon` 子命令（本地）

| 命令 | 行为 |
|---|---|
| `daemon status [--json]` | 本地 health + pid + uptime + data dir；`--host` 时看远端 |
| `daemon config [--json]` | 打印数据目录、hosts.json 路径、并发上限、版本 |
| `daemon start [--bind --data]` | 后台拉起自身（Unix `setsid` + `nohup` 语义用 `Command::new(current_exe).arg("serve")` + `pre_exec(setsid)`；Windows 用 `DETACHED_PROCESS|CREATE_NO_WINDOW`），写 pid 文件 `roamway.pid` |
| `daemon stop` | 读 pid → SIGTERM（Windows `taskkill /T /F`）→ 等 5s → 校验端口已释放 |
| `daemon restart` | stop + start |
| `daemon reload` | 重新加载 hosts.json 与 state，不重启进程（SIGHUP 之外：直接重读即可） |
| `daemon pair` | 打印 `http://<ip>:<port>?token=<token>` 的一行配对串（token 用 mask 之外的完整值，因为这是给用户自己用的） |

顶层 `restart` / `reload` = `daemon restart` / `daemon reload`，支持 `--host`。

## 5. `onboard`

`roamway onboard`：打印
1. 生成 token（32 hex）并写入 `<data>/token`（0600）
2. 打印启动命令（前台 / 后台 / crontab 三行）
3. 打印从另一台机器 `host add` 的完整命令

## 6. 单元测试（必须新增）

- `endpoint_normalize`：三种输入归一化一致
- `registry_roundtrip`：add → save → load 字段一致
- `registry_corrupt_recovers`：写坏 JSON → load 空且备份文件存在
- `name_validation`：大写/空格/超长 被拒
- `backoff_caps_at_3600`
- `mask_short_and_long`
- `get_by_name_or_endpoint`

## 7. 验收（NAS 上执行）

1. PC `roamway host add nas --endpoint 10.0.0.91:8796 --token $T` → `host ls --probe` 显示 `online` + 版本 + 延迟
2. `roamway run --host nas --agent shell --prompt "echo ok"` → NAS 侧 `roamway ls` 能看到同一条 run
3. `roamway ls --host all` → 合并 PC + NAS，字段含 `host`
4. 杀掉 NAS daemon → `host ls` 显示 `error` + `retry` 秒数；重启后自动恢复 `online`
5. `hosts.json` 权限 600，`host ls` 不出现明文 token
6. `daemon status/start/stop/restart` 全部可用，stop 后端口释放
7. `cargo build --release` 0 warning，`cargo test` 全过

## 8. 约束

- 不改既有 13 条 HTTP 路由语义；只新增 `GET /v1/hosts`（返回注册表，token 已 mask）。
- 所有新代码 `cargo fmt` 后 0 warning。
- 不打印任何 token 明文（`daemon pair` 除外）。
- Windows 分支也要能编译（CI 三平台）。
