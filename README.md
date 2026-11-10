# roamway

[![CI](https://github.com/ValueMoonLab/roamway/actions/workflows/ci.yml/badge.svg)](https://github.com/ValueMoonLab/roamway/actions/workflows/ci.yml)
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
git clone https://github.com/ValueMoonLab/roamway.git
cd roamway
cargo build --release
./target/release/roamway --help

# 2. 起 daemon（数据默认 ~/.roamway）
./target/release/roamway serve --bind 127.0.0.1:8796 --token mysecret

# 3. 派任务（另一个终端）
ROAMWAY="./target/release/roamway --token mysecret"
$ROAMWAY run  --agent shell --title hello "echo hi from roamway"
$ROAMWAY ls                 # 默认 agent 维度；--runs 看运行记录
$ROAMWAY logs run_000001
```

`run` 默认前台等待并打印尾部日志；长任务加 `--no-wait`，只拿 run id 就走。`ls` 默认列出 agent（`ID/STATE/KIND/...`），要回看历史运行记录用 `ls --runs`。


worktree 隔离示例（在独立分支跑，不污染当前工作区）：

```bash
$ROAMWAY run --new-workspace worktree --label stage=p12 --env FOO=1 --agent shell "echo hi"
```

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

### 预编译产物

每个 tag（`v*`）会自动构建三平台产物并挂到 GitHub Releases。Windows 用户下载
`roamway-v*-windows-x64.zip`，解压即用：

```powershell
# 解压后
.\roamway.exe --version
pwsh scripts\smoke.ps1   # Windows 端到端冒烟
```

### 系统要求

Linux / macOS / Windows（Git Bash 或 PowerShell 均可）。Linux 2 核 2G 足够跑十几个并发 agent —— 真正的开销在模型侧，不在 daemon。

`codex` / `hermes` / `claude` 是可选依赖，只装你要用的那个；`shell` 和 `api:*` 不需要任何 CLI。

#### macOS

- Apple Silicon 使用 `aarch64-apple-darwin`，Intel 使用 `x86_64-apple-darwin`，都走同一份 `cargo build --release`。
- 国内用 rsproxy.cn 镜像实测：rustup 安装约 24 秒，首次构建约 95 秒。
- 若 git 全局配置里残留失效的 `http.proxy`，cargo 拉取依赖时会报 spurious network error；在 `~/.cargo/config.toml` 里加 `[http] proxy = ""` 可解决。

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

命令面已与 Paseo 对齐（31 个顶层命令 / 96 条），不再手工维护命令清单，权威文本随时由命令自身输出：

```bash
roamway help              # 总帮助
roamway help <cmd>        # 任意命令帮助（与 roamway <cmd> --help 完全一致）
roamway <cmd> --help      # 同上
```

核心命令速览（完整清单以 `roamway help` 为准）：`serve`、`start`、`run`、`send`、`delete`、`ls`、`status`、`logs`、`stop`、`rm`、`schedule`、`heartbeat`、`permit`、`workspace`、`agent`、`daemon`、`host`、`hub`、`terminal`、`script`、`provider`、`usage`、`plugin`、`speech`。

全局参数：`--host http://127.0.0.1:8796`、`--token T`、`-o/--format table|json|yaml`、`--json`、`-q/--quiet`、`--no-headers`、`--no-color`、`--home DIR`（等价 `--data DIR`）；环境变量 `ROAMWAY_HOST` / `ROAMWAY_TOKEN` 仍可用。

`run` 传图可用 `--image PATH` 或短别名 `-i PATH`。未知短标志（例如 `-x`）在解析期报错并 exit 2；prompt 里以 `-` 开头的词请放在 `--` 之后。

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

出于安全考虑，`agent` 采用白名单：只接受 `codex`、`hermes`、`claude`、`shell`、`api:deepseek`、`api:qwen`。要接入新 CLI，请在 `src/types.rs` 的 `KNOWN_AGENTS` 与 `src/runner.rs` 的 `build_command` 里各加一条映射后重新编译。

### 每 run 参数透传

`run` 支持把思考档位、沙箱模式、provider、图片、输出 schema 直接交给该 run 的 agent（映射逐 provider 见 `docs/providers.md`）：

| 参数 | 取值 | codex 实际 argv |
|---|---|---|
| `--thinking` | `none` `low` `medium` `high` | `-c model_reasoning_effort=<level>` |
| `--mode` | `read-only` `workspace-write` `danger-full-access` | `-s <mode>`（不给则仍是 `workspace-write`） |
| `--provider` | `ollama` / `lmstudio` / `oss` / 其它 id | `--oss --local-provider <P>` / `--oss` / `-c model_provider=<P>` |
| `--image <FILE>` / `-i <FILE>` | 可重复，每个都是存在的文件 | 每张一个 `-i <FILE>` |
| `--output-schema <FILE>` | 存在的 JSON 对象文件（须含 `type`） | `--output-schema <FILE>` |

```bash
roamway run --agent codex --thinking high --mode read-only \
  --image shot.png --output-schema schema.json --provider ollama "看一下这张图"
```

校验在 CLI 与 HTTP 各做一次，文案逐字一致：

- `--thinking` / `--mode` 只接受枚举：`run --thinking must be one of none|low|medium|high`、`run --mode must be one of read-only|workspace-write|danger-full-access`
- `--image` 必须是存在的文件：`run --image: no such file: <p>`
- `-i PATH` ≡ `--image PATH`：文件校验与错误文案完全一致
- `--output-schema` 必须是存在的文件且内容是带 `type` 的 JSON 对象：`run --output-schema: not a JSON object with type: <p>`
- `--provider` 限 `[A-Za-z0-9_.-]{1,40}`
- 未知 key：`unknown run option: <k>`

不支持面：`claude` 没有 provider / image / output-schema 的能力，带上是 `run --provider: not supported for agent claude` 一类错误；它的 `--thinking` 走环境变量 `MAX_THINKING_TOKENS`（low=2048 / medium=8192 / high=24576，`none` 不设），`--mode` 走 `--permission-mode`。`shell`、`hermes`、`api:*` 带任何选项一律 `run --<k>: not supported for agent <kind>`。所有被拒的参数在派发前就判掉，不会出现「跑完了才发现参数没生效」。

HTTP 侧同构：`POST /v1/runs` 与 `POST /v1/agents/{id}/run` 接受 `"options": {"thinking":"high","mode":"read-only","provider":"ollama","output_schema":"/abs/schema.json"}` 与 `"images": ["/abs/a.png"]`，非法时 400 `{"error": "<同上文本>"}`；`GET /v1/runs/{id}` 与 `run --json` 原样回显这两项。daemon 日志每 run 打一行 `[runopts] agent=<kind> keys=<k1,k2> images=<n>`（只打 key，不打值）。

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

`GET /health`、`POST /v1/auth/login`、`POST /v1/auth/pair` 免鉴权；其余路由在有鉴权配置时只接受 `x-roamway-token: <T>` 请求头（SSE 事件流仍允许 `?token=`）。无 token 且无多 token 时是开放模式，不要求鉴权。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 免鉴权；`{ok, version, uptime_s, running, queued}` |
| GET | `/v1/health` | 需鉴权；版本、各状态 run/job 计数、数据目录 |
| GET | `/v1/runs?limit=50&offset=0` | 最近 run（倒序）；`limit` 1..=200，不返回 prompt 全文 |
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
| POST | `/v1/auth/login` | `{password}` → 24h 短期 token（需先 `daemon set-password`） |
| POST | `/v1/auth/pair` | `{code}` → 长期 token（`pair` 生成 6 位码，60s 一次性） |
| GET | `/metrics` | Prometheus 文本（`text/plain; version=0.0.4`）；需 token，`?token=` 可用 |
| GET | `/v1/approvals?all=1` | 审批列表（默认仅 pending，`all` 含已决） |
| POST | `/v1/approvals/{id}/approve` / `deny` | 批准 / 拒绝（需 admin scope） |

`POST /v1/runs/{id}/stop` 对运行中的 `api:*` run 返回 `409 {"error":"api run not cancellable"}`（直连 API 无法中断）；`queued` 的 `api:*` run 仍可取消，其余 run 会先写取消标志再改状态，避免停止后复活。

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
| 可观测 | `~/.roamway/heartbeat` 每 5 秒刷新；`/health` 与 `/v1/health` 汇总计数 |

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
roamway serve --bind 0.0.0.0:8796 --token T --allow-host 192.168.1.50

# B 机派活到 A
roamway run --host http://192.168.1.50:8796 --token T \
  --agent codex --workspace /srv/repo "跑一下测试"   # 192.168.1.50 换成你的 daemon 地址
```

哪台机器有数据、有 GPU、有专用凭据，就把 daemon 放哪台。公网暴露请放在 TLS 反代后面。

## Web 控制面

`roamway serve` 启动后，浏览器访问 `http://127.0.0.1:8796/ui/` 即可打开控制面；入口也可以由 `roamway ui [--print]` 打印。

- 登录：页面输入 daemon token 后存入 `localStorage`，所有 API 请求带 `Authorization: Bearer`，401 自动回到登录页。
- 页面：Agents 列表/详情与实时输出（SSE）、worktree diff、Hosts 管理与 probe、Schedules、Terminals、Usage、Settings（token / provider / 主题）。
- 移动端：支持深色模式与 720px 断点，关键操作 44px 最小点击区。
- 静态资源随二进制嵌入（`include_str!`）：`/ui/` 返回 `index.html`，`/ui/app.js` 与 `/ui/style.css` 同源内嵌，无需 Node 构建链；`GET /` 302 到 `/ui/`。
- 截图：暂不随仓库提交二进制截图；界面以源码为准（`src/web/index.html`、`src/web/app.js`）。

```bash
roamway serve --bind 127.0.0.1:8796 --token mysecret   # 启动后打开 http://127.0.0.1:8796/ui/
```

## 安全

- 默认只绑 `127.0.0.1`；`--bind 0.0.0.0` 是你主动开放给局域网。
- token 走明文 HTTP，只适合可信内网；公网请套 TLS 反代（Caddy / Nginx / frp + TLS 都行）。`--tls-cert/--tls-key` 当前为占位参数，TLS 直连尚未实现。
- 鉴权分三层：无鉴权配置时开放模式；配置后无 token/无效 token 返回 `401`；scope 不足返回 `403`（`read`/`run`/`admin`，admin 包含全部）。
- 多 token 落在 `<data>/tokens.json`，只存 sha256；密码只存 PBKDF2-HMAC-SHA256（`password.json`），配对码 6 位数字 60s、一次性消费。
- `--allow-host` 可多次，配了 Host 白名单后不匹配返回 `421`；未配置时只允许 loopback 与显式 bind 地址。
- CORS：只有配 `--allow-origin` 才发 `Access-Control-Allow-Origin`，并正确处理 `OPTIONS` 预检；所有响应带 `X-Content-Type-Options`/`X-Frame-Options`/`Referrer-Policy` 安全头。
- `--require-approval run|admin`：命中 scope 的写请求先返回 `202` 入审批队列；`approve` 后用 `X-Approval: <id>` 重放且 body hash 一致才放行，改 body 返回 `409`；审批记录保留 7 天。
- `tokens.json`/`password.json`/`approvals.json` 以 0600 落盘（Windows 忽略该位）；日志与 `token ls` 不输出明文 token 与密码。
- 没设 token 且 `--bind` 非 loopback（`127.0.0.1`/`::1`）时 daemon 拒绝启动（退出码 2）；只在 loopback 绑定下才允许无 token 运行。
- `codex` 默认 `workspace-write` 沙箱。加 `--dangerously-bypass-approvals-and-sandbox` 等于把整台机器交给模型，别在有重要数据的主机上这么干。
- `--token` 会把 token 写进进程命令行（`ps` 可见）。长期部署请用 `ROAMWAY_TOKEN` 环境变量或 `EnvironmentFile=`，`scripts/roamway-start.sh` 已按这种方式处理。
- 数据目录里有全量日志，按需 `chmod 700`。

## 监控

`GET /metrics` 输出 Prometheus 文本格式（exposition 0.0.4），覆盖构建信息、运行时长、各状态 run 数、agent、job、调度、终端、并发连接与 HTTP 请求计数：

| 指标 | 类型 | 说明 |
|---|---|---|
| `roamway_build_info{version}` | gauge | 恒为 1，`version` 是编译时的 crate 版本 |
| `roamway_uptime_seconds` | gauge | 进程已运行秒数 |
| `roamway_runs{status}` | gauge | 7 种状态各一行（queued/running/done/failed/timeout/killed/interrupted） |
| `roamway_agents{state}` | gauge | 按 agent 状态分组；无 agent 时输出 `roamway_agents 0` |
| `roamway_jobs` / `roamway_jobs_enabled` | gauge | 定时任务总数 / 启用数 |
| `roamway_schedules` / `roamway_schedules_paused` | gauge | 调度总数 / 已暂停数 |
| `roamway_schedule_failures_total` | counter | Σ `consecutive_failures`（daemon 内存态，重启归零） |
| `roamway_terminals` | gauge | 活动终端数 |
| `roamway_http_concurrent_requests` | gauge | 当前在处理的连接数 |
| `roamway_http_requests_total` | counter | 累计请求数（内存态，重启归零） |
| `roamway_http_requests_failed_total` | counter | 累计响应码 ≥ 400 的请求数 |

`/metrics` **不豁免鉴权**（`/health` 仍是唯一免鉴权端点）：`Authorization: Bearer <token>` 或 `?token=<token>` 二选一。给 Prometheus 建一个只读长期 token：

```bash
roamway token add --name prometheus --scope read --ttl 0   # 打印一次明文，记进 scrape 配置
```

最小 scrape 配置：

```yaml
scrape_configs:
  - job_name: roamway
    metrics_path: /metrics
    # 方式一：Bearer（推荐，token 不出现在 URL 与访问日志里）
    authorization:
      type: Bearer
      credentials: "<prometheus token>"
    # 方式二：查询参数（等价，方便 curl 手查）
    # params:
    #   token: ["<prometheus token>"]
    static_configs:
      - targets: ["127.0.0.1:8796"]
```

只读 scope 足够抓取；`/metrics` 只读内存状态，不碰磁盘、不触发任务。曲线建议先盯 `roamway_runs{status="failed"}` 与 `roamway_schedule_failures_total` 的增速。

## 和 Paseo 的差别

| | Paseo | roamway |
|---|---|---|
| 定位 | 人在环的多 agent 工作台 | 无人值守的任务执行器 |
| 界面 | Web UI / WSS | HTTP + CLI |
| 多机 | 联邦、host 列表 | 各自独立 daemon，互相调 |
| 定时任务 | 0.15+ 支持 cron schedule | 有，带退避与上限 |
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

- [x] 参数面对齐完成（P15+P16）
- [x] Windows 原生预编译产物
- [x] 任务依赖（A 成功后再跑 B）
- [x] 并发上限（全局 / 每 agent）
- [x] `/metrics`（Prometheus 文本格式）
- [ ] 只读 Web 面板

## License

MIT，见 [LICENSE](LICENSE)。
