# 配置与数据目录

## 数据目录

默认 `<home>/.roamway/`（Linux/macOS 为 `~/.roamway`，Windows 为 `%USERPROFILE%\.roamway`）。可用 `--data DIR` 或 `--home DIR`（等价）覆盖。

| 文件/目录 | 作用 |
|---|---|
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
- `2` 未知命令、未知子命令、缺少必填参数、`project`（不支持）