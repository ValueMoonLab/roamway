# 配置与数据目录

## 权威配置表（config.json）

路径：`<data_dir>/config.json`，全部字段可选，缺省值由代码内置。优先级：CLI flag > `ROAMWAY_*` 环境变量 > config.json > 内置默认。

| key | 类型 | 默认 | 环境变量 | 说明 / 示例 |
|---|---|---|---|---|
| `bind` | string | `127.0.0.1:8796` | `ROAMWAY_BIND` | 监听地址，必须带端口；`--bind 0.0.0.0:8796` |
| `max_concurrent` | u64 | `4` | `ROAMWAY_MAX_CONCURRENT` | 最大并发 run，`>0`；`--max-concurrent 8` |
| `max_conn` | u64 | `64` | `ROAMWAY_MAX_CONN` | HTTP 最大连接数，`>0`；`--max-conn 128` |
| `tz_offset_hours` | i64 | `8` | `ROAMWAY_TZ` | 本地时区偏移（小时）；`--tz 9` |
| `unix_socket` | string? | 无 | — | unix socket 路径；`--unix-socket /tmp/rw.sock` |
| `tls_cert` | string? | 无 | — | TLS 证书路径；`--tls-cert /path/cert.pem` |
| `tls_key` | string? | 无 | — | TLS 私钥路径；`--tls-key /path/key.pem` |
| `allow_hosts` | string[] | `[]` | `ROAMWAY_ALLOW_HOSTS`（逗号分隔） | 允许的来源主机；`--allow-host a.example --allow-host b.example` |
| `allow_origins` | string[] | `[]` | — | 允许的 CORS 来源；`--allow-origin https://x` |
| `require_approval` | bool | `false` | `ROAMWAY_REQUIRE_APPROVAL`（`1/true/0/false`） | 是否要求 run/admin 审批；`--require-approval run`（可重复） |
| `token_env` | string | `ROAMWAY_TOKEN` | — | token 所在的环境变量名（只存名字，不存值） |
| `keep_runs` | u64 | `500` | — | 保留的最新 terminal run 数；运行期与启动期同一阈值，`0`=不修剪 |
| `max_log_bytes` | u64 | `4194304`（4 MiB） | — | 单 run 日志字节上限；`0`=不限 |

`daemon config set` 用逗号分隔写数组（`allow_hosts=a,b`）；`daemon config unset <key>` 回到 env/default 生效。

## token 三级回退

1. CLI `--token T`
2. 环境变量 `ROAMWAY_TOKEN`
3. `<data_dir>/token` 文件

**token 永不写入 config.json**：config.json 只允许 `token_env`（环境变量名），出现 `token` / `password` / `secret` / `api_key` 等键直接报错并 exit 2。

## 数据目录

默认 `<home>/.roamway/`（Linux/macOS 为 `~/.roamway`，Windows 为 `%USERPROFILE%\.roamway`）。可用 `--data DIR` 或 `--home DIR`（等价）覆盖。

| 文件/目录 | 作用 |
|---|---|
| `config.json` | 持久部署配置（本表） |
| `bind` | 最后一次 `serve`/`daemon start` 的实际监听地址 |
| `token` | 单 token 模式下的服务端 token |
| `hosts.json` | 主机注册表（`roamway host add/ls/rm/enable/disable/probe`） |
| `tokens.json` | 多 token（只存 sha256） |
| `password.json` | `daemon set-password` 的 PBKDF2 哈希 |
| `workspaces.json` | 工作区注册表（`workspace create/ls/archive`） |
| `schedules.json` | 调度存储（`schedule` / `heartbeat` 共用） |

损坏的 `hosts.json` 会被改名备份为 `hosts.json.corrupt-<ts>`，然后重新加载空注册表。

## 环境变量

| 变量 | 作用 |
|---|---|
| `ROAMWAY_HOST` | 客户端默认连接地址 |
| `ROAMWAY_TOKEN` | 服务端 / 客户端 token |
| `ROAMWAY_BIND` | `bind` 覆盖 |
| `ROAMWAY_MAX_CONCURRENT` | `max_concurrent` 覆盖 |
| `ROAMWAY_MAX_CONN` | `max_conn` 覆盖 |
| `ROAMWAY_TZ` | `tz_offset_hours` 覆盖 |
| `ROAMWAY_ALLOW_HOSTS` | `allow_hosts` 覆盖（逗号分隔） |
| `ROAMWAY_REQUIRE_APPROVAL` | `require_approval` 覆盖（`1/true/0/false`） |
| `ROAMWAY_DEEPSEEK_KEY` | `api:deepseek` 的 key |
| `ROAMWAY_QWEN_KEY` | `api:qwen` 的 key |
| `ROAMWAY_DEEPSEEK_URL` | 覆盖 DeepSeek 端点 |
| `ROAMWAY_QWEN_URL` | 覆盖 Qwen 端点 |
| `ROAMWAY_CODEX_ARGS` | 追加给 `codex exec` 的参数 |
| `ROAMWAY_HERMES_ARGS` | 追加给 `hermes` 的参数 |

## 全局参数

`--host`、`--token`、`-o/--format table|json|yaml`、`--json`（等价 `--format json`）、`-q/--quiet`、`--no-headers`、`--no-color`、`--home DIR`（等价 `--data DIR`）。`--help/-h` 在副作用前短路，`roamway help <cmd>` 与 `roamway <cmd> --help` 输出同一份文本。

## 退出码

- `0` 成功
- `1` 网络/远端失败
- `2` 未知命令、未知子命令、缺少必填参数、非法配置值、`project`（不支持）