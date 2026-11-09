# P14_SPEC — daemon 部署面（持久 config.json / config get·set·unset / daemon run）

前置事实（实测于 P13 之后的 HEAD）：

- `src/main.rs:3351 fn cmd_daemon_config` 只**打印**四行：`data_dir`、`hosts`、`concurrency`（来自 `--max-concurrent`，
  缺省硬编码 4）、`version`。没有配置文件、没有 get/set/unset，也读不到守护进程实际在跑的参数。
- `src/main.rs:3386 fn cmd_daemon_start` 只吃 `--bind`（缺省 `127.0.0.1:8796`）与 `--token`/`ROAMWAY_TOKEN`，
  其余参数（`--data`、`--tz`、`--max-concurrent`、`--max-conn`、`--allow-host`、`--require-approval`、
  `--unix-socket`、`--tls-cert/--tls-key`）**无法传给守护进程**——`cmd_serve` 能认（`main.rs:827-829`），
  `start` 不转发。每次重启都要重打一遍命令行，做不到"部署一次、复现多次"。
- 无 `daemon run` 子命令：`daemon` 只认 `status|config|start|stop|restart|reload|pair|set-password`
  （`main.rs:3273-3284`）。前台可用的只有顶层 `serve`，参数解析路径与 `start` 不同。
- `data_dir_for`（`main.rs:432`）只认 `--data`，否则 `default_data_dir()`；`local_token`（`main.rs:396`）
  已是三级回退 flag → `ROAMWAY_TOKEN` → `data/token` 文件，这部分**正确，不要改**。
- `daemon set-password`（`cmd_daemon_set_password` → `auth.rs`，pbkdf2-sha256）已实现，本阶段不重做。
- PARITY ⚠️#15（`paseo daemon run`）/ ⚠️#63（daemon 配置面）就是本阶段要关的缺口。

本阶段目标：让 daemon 的部署参数**持久、可查、可改、可复现**，并且 token 绝不落进配置文件。
只新增能力，不改 `serve` 现有参数语义与 `/health` 响应。

---

## 一、配置文件（新模块 `src/config.rs`）

1. 路径：`<data_dir>/config.json`（`data_dir` 由 `--data`/`--home`/默认决定，不另开位置）。
2. 结构（全部可选，缺省值在代码里）：
   ```json
   {
     "bind": "127.0.0.1:8796",
     "max_concurrent": 4,
     "max_conn": 64,
     "tz_offset_hours": 8,
     "unix_socket": null,
     "tls_cert": null,
     "tls_key": null,
     "allow_hosts": [],
     "allow_origins": [],
     "require_approval": false,
     "token_env": "ROAMWAY_TOKEN",
     "keep_runs": 500
   }
   ```
   - **禁止**出现 token / password / 任何密钥值。只允许 `token_env`（环境变量名）。
     解析时若发现未知 key 或疑似密钥字段（`token`、`password`、`secret`、`api_key`），
     直接报错 `config.json contains forbidden key 'token'`（exit 2）——防止误存。
   - `keep_runs` 供 `store` 修剪历史 run（当前无上限，按此值实现 trim，默认 500，0 = 不修剪）。
3. 读取必须容忍文件缺失（返回默认）、JSON 语法错误（stderr 明确 `config.json: bad json at line N`，exit 2）。
   写入用 tmp + rename 原子替换；unix 上 `0o600`，Windows 上不设权限但不得写进共享目录。

## 二、优先级与解析（`src/main.rs`）

1. 统一解析函数 `fn resolve_config(flags, env, file) -> EffectiveConfig`，优先级
   **CLI flag > 环境变量 `ROAMWAY_*` > config.json > 内置默认**。
   受支持的环境变量：`ROAMWAY_BIND`、`ROAMWAY_MAX_CONCURRENT`、`ROAMWAY_MAX_CONN`、`ROAMWAY_TZ`、
   `ROAMWAY_ALLOW_HOSTS`（逗号分隔）、`ROAMWAY_REQUIRE_APPROVAL`（`1/true/0/false`）。
2. `cmd_serve`、`cmd_daemon_start`、新 `cmd_daemon_run` 全部走这同一个 `resolve_config`，
   禁止三处各自写默认值。`daemon start` 把解析结果**显式**转成子进程参数（或子进程 env），
   不得依赖"子进程自己再解析一遍"这种隐性耦合——但子进程仍然会解析，二者结果必须一致（用测试锁住）。
3. 非法值（`max_concurrent=0`、`bind` 无端口、`tz=abc`）→ stderr 指明 key 与非法值，exit 2。

## 三、CLI（src/main.rs:cmd_daemon*）

1. `daemon config`（无子参数）→ 打印 effective config，**带来源列**：
   ```
   bind             127.0.0.1:8796   file
   max_concurrent   4                default
   tz_offset_hours  8                env
   ```
   `--json` 输出 `{"config":{...},"sources":{key:"flag|env|file|default"}}`。
2. `daemon config get <key>` → 只打值（`--json` 打 `{"key":..,"value":..,"source":..}`）；未知 key → exit 2。
3. `daemon config set <key> <value>` → 校验后写 config.json，打印 `set <key> = <value>`；
   未知 key / 非法值 → exit 2 且**不改文件**。数组型用逗号分隔（`allow_hosts=a,b`）。
4. `daemon config unset <key>` → 从文件删除该 key（回到 env/default 生效），打印 `unset <key>`；不存在也算成功。
5. `daemon run [--foreground]`：前台 serve，参数走 `resolve_config`；等价于"按配置起生产"。
   `daemon run` 是 Paseo `daemon run` 的对应命令，必须进 `usage()` 与 per-command help。
6. `daemon status` 增加 `config_file` 与 `effective_bind`（JSON 与表格都加，表格放末行 `config <path>`）；
   `daemon start` 成功后打印 `daemon started pid=<> bind=<> config=<path>`。

## 四、单测

1. `resolve_config`：三组用例锁优先级（flag>env>file>default 各一条；解析 env 逗号数组；非法值 Err）。
2. `Config::load` / `Config::save` 往返：写入后读回逐字段相等；文件内容不含 `"token"` 字样（断言字符串）。
3. `fn config_key_allowed(key: &str) -> bool` 白名单：`bind/max_concurrent/...` 为 true，`token/password/foo` 为 false。
4. `fn apply_set(cfg, key, value) -> Result<(), String>`：`max_concurrent=0` Err；`tz=9` ok；`allow_hosts=a,b` 变 2 元。
5. `keep_runs` 修剪：构造 3 条 run、`keep_runs=2`，调用修剪函数后剩最新 2 条且不误删 running 状态的 run。

## 五、文档

- `docs/config.md`：改成权威配置表（key / 类型 / 默认 / 环境变量 / 说明 / 示例），写明 token 的三级回退与
  "token 永不写入 config.json"。
- `docs/quickstart.md`：加「部署一次、复现多次」三行：`daemon config set bind …` → `daemon config set max_concurrent 8`
  → `daemon start`。
- `PARITY.md`：#15（`daemon run`）改 ✅ 或 ⚠️（若 `--thinking` 类参数未对齐则 ⚠️ 并写理由）；#63 改 ✅，
  证据写 `src/config.rs:fn resolve_config` 与测试名；把"Paseo daemon config 选项全集"逐条对表，不做项写理由。

## 六、验收（keeper 实测，NAS）

- NAS `cargo build --release && cargo test` 全绿、0 warning。
- 在 `/home/GZX/.roamway` 的**副本**目录上实测（不动生产数据）：
  - `roamway --data /tmp/rwcfg daemon config` → 每行带来源；`--json` 里 `sources.bind == "default"`
  - `roamway --data /tmp/rwcfg daemon config set bind 127.0.0.1:8899` → 文件里出现 `"bind": "127.0.0.1:8899"`，
    `daemon config get bind` 回显该值；`daemon config set token abc` → rc=2 且文件未变
  - `ROAMWAY_MAX_CONCURRENT=9 roamway --data /tmp/rwcfg daemon config get max_concurrent` → 9 且 source=env
    （证明 env 高于 file，flag 高于 env）
  - `roamway --data /tmp/rwcfg daemon run --bind 127.0.0.1:8797` 前台起来后 `curl /health` 200，Ctrl-C 可退
  - `config.json` 全文 `grep -c token` = 0（除 `token_env` 键名外不得出现 token 值）
- 生产 NAS 8796 不受影响：跑完后 `roamway daemon status --host nas` 仍 online（pid 不变）。
