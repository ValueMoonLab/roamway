# roamway

[![CI](https://github.com/ValueMoon-Lab/roamway/actions/workflows/ci.yml/badge.svg)](https://github.com/ValueMoon-Lab/roamway/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

**为国内网络、长期无人值守场景设计的极简多智能体编排器。** 一个二进制、一个 HTTP 接口、一台机器就能跑；要更多机器就多起几个 daemon。

```
        ┌──────────────┐   HTTP/JSON    ┌──────────────┐
        │  CLI / curl  │ ─────────────▶ │   roamway    │
        └──────────────┘   x-token      │    daemon    │
                                        └──────┬───────┘
                        ┌──────────────────────┼──────────────────────┐
                        ▼                      ▼                      ▼
                   codex CLI              hermes CLI           DeepSeek / Qwen
                 (写代码、改仓库)        (通用任务、带工具)        (直连 HTTP API)
```

## 为什么又造一个

[Paseo](https://paseo.sh) 很好，但它面向"人在电脑前盯着多个 agent"：Web UI、WSS、多 host 联邦、worktree 管理。真把 agent 丢去跑一整夜时，缺的是另外几样东西：

- 进程崩了会不会自己起来，起来后会不会把半截任务**重跑一遍**（重复扣费、重复写库）；
- 任务失败后是无限重试刷屏，还是**退避几次就停**；
- 同一任务上一轮没跑完，下一轮会不会**叠上来**；
- 日志和状态记录会不会**无限膨胀**把磁盘吃满。

roamway 只做这些，外加一个够用的调度器和 HTTP 接口。约 1,000 行 Rust，2 MB 二进制，没有 Node、没有 Python、没有数据库。

## 特性

- **单二进制**：`cargo build --release` 出一个文件，拷过去就能跑，零运行时依赖。
- **国内可用**：默认只连 `api.deepseek.com` 与 `dashscope.aliyuncs.com`；构建走 rsproxy / 清华镜像；运行不依赖 GitHub。
- **长跑优先**：重启不重放、失败指数退避、重试上限后自停、同一任务不重叠、记录有界、心跳文件。
- **接现成的 CLI**：`codex` / `hermes` / `claude` 直接复用你已登录的凭据，不用把 key 再抄一遍。
- **也能直连 API**：`api:deepseek` / `api:qwen`，地址可覆盖（中转、内网网关、Ollama 兼容端点都行）。
- **HTTP + token**：任何语言、任何机器都能派活，客户端不必装。
- **定时任务**：`every_secs` 或 `daily HH:MM`，可配时区偏移。
- **无厂商锁定**：只要命令行能跑、退出码可信，就能当 agent。

## 快速开始

```bash
# 1. 构建（Rust 1.75+）
git clone https://github.com/ValueMoon-Lab/roamway.git
cd roamway
cargo build --release
./target/release/roamway --help

# 2. 起 daemon（数据默认 ~/.roamway）
./target/release/roamway serve --bind 127.0.0.1:8796 --token mysecret

# 3. 派任务（另一个终端）
ROAMWAY="./target/release/roamway --token mysecret"
$ROAMWAY run  --agent shell --title hello "echo hi from roamway"
$ROAMWAY ls
$ROAMWAY logs run_000001
```

`run` 默认前台等待并打印尾部日志；长任务加 `--no-wait`，只拿 run id 就走。

## 安装

### 源码构建

```bash
cargo build --release
install -m 755 target/release/roamway /usr/local/bin/roamway
```

国内首次构建建议先配镜像（`~/.cargo/config.toml`）：

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

[net]
git-fetch-with-cli = true
```

rustup 本身用清华源：

```bash
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
export RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
```

### 系统要求

Linux / macOS / Windows（Git Bash 或 PowerShell 均可）。Linux 2 核 2G 足够跑十几个并发 agent —— 真正的开销在模型侧，不在 daemon。

`codex` / `hermes` / `claude` 是可选依赖，只装你要用的那个；`shell` 和 `api:*` 不需要任何 CLI。

## 核心概念

| 概念 | 含义 |
|---|---|
| **daemon** | 一台机器上的常驻进程，监听 HTTP，管理本机所有 run 与 job |
| **run** | 一次 agent 执行：状态、exit code、起止时间、日志文件 |
| **job** | 定时任务：每 N 秒或每天某时刻触发一个 run |
| **agent** | 执行方式：`codex`、`hermes`、`claude`、`shell`、`api:deepseek`、`api:qwen` |
| **workspace** | run 的工作目录（子进程 cwd，也是 codex 的 `-C`） |

状态机：

```
queued ──▶ running ──▶ done
                 ├───▶ failed
                 ├───▶ timeout
                 ├───▶ killed
                 └───▶ interrupted   (daemon 重启，不重放)
```

## CLI

```
roamway serve    [--bind 127.0.0.1:8796] [--data DIR] [--token T] [--tz 8]
roamway run      --agent AGENT [--model M] [--workspace DIR] [--title T]
                 [--timeout 1800] [--no-wait] "prompt"
roamway ls       [--limit 20]
roamway status   RUN_ID
roamway logs     RUN_ID [--tail 80]
roamway stop     RUN_ID
roamway rm       RUN_ID
roamway jobs
roamway jobs-add --agent AGENT (--every 3600 | --daily 08:30)
                 [--model M] [--workspace DIR] [--max-retries 2]
                 [--allow-overlap] [--disabled] "prompt"
roamway jobs-run JOB_ID
roamway jobs-rm  JOB_ID
roamway health
```

通用参数：`--host http://127.0.0.1:8796`、`--token T`（也可用 `ROAMWAY_HOST` / `ROAMWAY_TOKEN`）。

## agent 类型

| agent | 实际执行 | 前置条件 |
|---|---|---|
| `codex` | `codex exec -s workspace-write --skip-git-repo-check -C <ws> --model M "<prompt>"` | 装好 codex 并已登录 |
| `hermes` | `hermes -z "<prompt>" -m M` | 装好 hermes |
| `claude` | `claude -p "<prompt>"` | 装好 claude 并已登录 |
| `api:deepseek` | POST `api.deepseek.com/chat/completions` | `ROAMWAY_DEEPSEEK_KEY` |
| `api:qwen` | POST `dashscope.aliyuncs.com/compatible-mode/v1/chat/completions` | `ROAMWAY_QWEN_KEY` |
| `shell` | `sh -c`（Windows 为 `cmd /C`） | 无 |

`codex` 默认给 `workspace-write` 沙箱——能改工作区，改不到外面。需要更大权限（比如装依赖）时：

```bash
ROAMWAY_CODEX_ARGS="--dangerously-bypass-approvals-and-sandbox" roamway serve ...
```

`hermes` 同理，用 `ROAMWAY_HERMES_ARGS` 追加参数（例如 `--yolo`）。

想接别的 CLI（aider、cline、自写脚本）：`agent` 填可执行文件名即可，prompt 作为唯一参数追加，退出码 0 视为成功。

## 环境变量

| 变量 | 作用 |
|---|---|
| `ROAMWAY_HOST` | 客户端默认连接地址 |
| `ROAMWAY_TOKEN` | 服务端 / 客户端 token |
| `ROAMWAY_DEEPSEEK_KEY` | `api:deepseek` 的 key |
| `ROAMWAY_QWEN_KEY` | `api:qwen` 的 key |
| `ROAMWAY_DEEPSEEK_URL` | 覆盖 DeepSeek 端点（中转 / 本地兼容服务） |
| `ROAMWAY_QWEN_URL` | 覆盖 Qwen 端点 |
| `ROAMWAY_CODEX_ARGS` | 追加给 `codex exec` 的参数 |
| `ROAMWAY_HERMES_ARGS` | 追加给 `hermes` 的参数 |

指向本地 Ollama（OpenAI 兼容）的例子：

```bash
ROAMWAY_DEEPSEEK_KEY=ollama \
ROAMWAY_DEEPSEEK_URL=http://127.0.0.1:11434/v1/chat/completions \
  roamway run --agent api:deepseek --model qwen3:8b "你好"
```

## HTTP API

除 `/health` 外都要带 `x-roamway-token: <T>` 头，或 `?token=<T>`。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 版本、各状态 run 数、job 数、数据目录 |
| GET | `/v1/runs?limit=50` | 最近 run（倒序） |
| POST | `/v1/runs` | 新建 run |
| GET | `/v1/runs/{id}` | run 详情 |
| GET | `/v1/runs/{id}/log?tail=200` | 日志尾部 |
| POST | `/v1/runs/{id}/stop` | 终止（kill 子进程） |
| DELETE | `/v1/runs/{id}` | 删除记录（运行中拒绝） |
| GET | `/v1/jobs` | 定时任务列表 |
| POST | `/v1/jobs` | 新建定时任务 |
| POST | `/v1/jobs/{id}/run` | 立即触发一次 |
| POST | `/v1/jobs/{id}/enable` / `disable` | 开关 |
| DELETE | `/v1/jobs/{id}` | 删除 |

派活：

```bash
curl -s -H "x-roamway-token: $TOK" -H 'content-type: application/json' \
  -d '{"agent":"codex","model":"deepseek-v4-pro","workspace":"/srv/repo",
       "title":"fix tests","prompt":"修好失败的测试并说明原因"}' \
  http://127.0.0.1:8796/v1/runs
```

定时任务：

```bash
curl -s -H "x-roamway-token: $TOK" -H 'content-type: application/json' \
  -d '{"agent":"shell","prompt":"/srv/bin/sync.sh","every_secs":3600,
       "max_retries":2,"timeout_s":900}' \
  http://127.0.0.1:8796/v1/jobs
```

## 定时任务与重试

- `every_secs`：按间隔触发；`daily: "HH:MM"`：每天某刻（`tz_offset_hours` 默认 8，即北京时间）。
- 上一轮没结束不会开下一轮，除非 `allow_overlap: true`。
- 失败（failed / timeout）按 `30s → 60s → 120s …` 退避重试，上限 1 小时。
- 重试次数超过 `max_retries` 后 job 自动 `enabled=false`，`roamway jobs` 里可直接看到。
- `max_retries=0` 表示不做额外重试，但按原周期继续跑（适合"每 5 分钟看一次，失败下次再说"）。

## 长跑保证

| 保证 | 实现 |
|---|---|
| 崩溃自愈 | systemd `Restart=always`，或 cron 每 2 分钟 `pgrep` 兜底 |
| 不重放半截任务 | 启动时把所有 `queued/running` 改判 `interrupted` |
| 不无限重试 | 指数退避 + 上限后自停 |
| 不重叠 | scheduler 检查同 job 是否有未结束的 run |
| 不撑爆磁盘 | 只保留最近 800 条 run，日志随之删除；单次读取封顶 200 KB |
| 单次不超时 | `timeout_s` 到点 kill，标 `timeout` |
| 可观测 | `~/.roamway/heartbeat` 每 5 秒刷新；`/health` 汇总计数 |

## 部署

### systemd

```ini
# /etc/systemd/system/roamway.service
[Unit]
Description=roamway multi-agent orchestrator
After=network-online.target

[Service]
User=roamway
Environment=ROAMWAY_TOKEN=change-me
Environment=PATH=/home/roamway/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/usr/local/bin/roamway serve --bind 127.0.0.1:8796 --data /var/lib/roamway
Restart=always
RestartSec=5
LimitNOFILE=8192
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

```bash
sudo useradd -r -m -d /var/lib/roamway roamway
sudo install -m 755 target/release/roamway /usr/local/bin/roamway
sudo systemctl enable --now roamway
```

无 sudo 时用用户级服务（`~/.config/systemd/user/` + `loginctl enable-linger $USER`），或 crontab：

```cron
@reboot      /home/you/bin/roamway-start.sh
*/2 * * * *  /home/you/bin/roamway-start.sh    # 幂等，已在跑就直接退出
```

`scripts/roamway-start.sh` 是一个可直接用的幂等启动脚本。

### Docker（可选）

```bash
docker build -t roamway .
docker run -d --name roamway -p 127.0.0.1:8796:8796 \
  -e ROAMWAY_TOKEN=change-me -v roamway-data:/data roamway
```

### 多机

roamway 不做联邦：一台机器一个 daemon，各自监听端口，跨机直接调 HTTP。

```bash
# A 机
roamway serve --bind 0.0.0.0:8796 --token T

# B 机派活到 A
roamway run --host http://192.168.1.50:8796 --token T \
  --agent codex --workspace /srv/repo "跑一下测试"   # 192.168.1.50 换成你的 daemon 地址
```

哪台机器有数据、有 GPU、有专用凭据，就把 daemon 放哪台。公网暴露请放在 TLS 反代后面。

## 安全

- 默认只绑 `127.0.0.1`；`--bind 0.0.0.0` 是你主动开放给局域网。
- token 走明文 HTTP，只适合可信内网；公网请套 TLS 反代（Caddy / Nginx / frp + TLS 都行）。
- 没设 token 时**任何人都能派活**，只在完全隔离的环境里这么用。
- `codex` 默认 `workspace-write` 沙箱。加 `--dangerously-bypass-approvals-and-sandbox` 等于把整台机器交给模型，别在有重要数据的主机上这么干。
- `--token` 会把 token 写进进程命令行（`ps` 可见）。长期部署请用 `ROAMWAY_TOKEN` 环境变量或 `EnvironmentFile=`，`scripts/roamway-start.sh` 已按这种方式处理。
- 数据目录里有全量日志，按需 `chmod 700`。

## 和 Paseo 的差别

| | Paseo | roamway |
|---|---|---|
| 定位 | 人在环的多 agent 工作台 | 无人值守的任务执行器 |
| 界面 | Web UI / WSS | HTTP + CLI |
| 多机 | 联邦、host 列表 | 各自独立 daemon，互相调 |
| 定时任务 | 无 | 有，带退避与上限 |
| 重启语义 | 取决于前端 | 明确不重放 |
| 体积 | Node 生态 | 2 MB 单文件 |

两者可以并存：白天用 Paseo 看过程，夜里用 roamway 跑批。

## 开发

```bash
cargo test              # 单元测试：时间计算、退避策略、状态机
cargo build --release
bash scripts/smoke.sh   # 端到端：起 daemon、跑 shell、验定时任务
```

代码结构：

```
src/main.rs     CLI 解析、子命令、HTTP 客户端
src/types.rs    数据结构、状态存储、时间工具
src/server.rs   HTTP 服务与路由
src/runner.rs   agent 适配与子进程生命周期
src/jobs.rs     调度器、退避、清理
docs/architecture.md  设计取舍
```

## Roadmap

- [ ] Windows 原生预编译产物
- [ ] 任务依赖（A 成功后再跑 B）
- [ ] 并发上限（全局 / 每 agent）
- [ ] `/metrics`（Prometheus 文本格式）
- [ ] 只读 Web 面板

## License

MIT，见 [LICENSE](LICENSE)。
