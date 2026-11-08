# roamway ↔ Paseo 功能对齐基线

基线：**Paseo 0.15.0**，CLI 共 31 个顶层命令 / **96 个叶子命令**（`paseo --help` 逐级导出，原始帮助文本见 `docs/paseo-cli-surface.md`）。
目标：**功能面 1:1 对齐**，roamway 作为 Paseo 的 Rust 实现可直接替代；不做减法。
图例：✅ 已对齐 · ⚠️ 部分对齐 · ❌ 未实现。阶段见文末排期。

> 本文件是验收基线，不是宣传材料。每完成一条，状态与证据链接同步更新，并由 NAS/PC/Mac 三端实测确认后才打 ✅。

## 一、CLI 命令对齐表（96 条）

| # | Paseo 命令 | 作用 | roamway | 差距 / 目标 | 阶段 |
|---|---|---|---|---|---|
| 1 | `paseo host` | Manage hosts connected to the selected | ❌ | 主机注册表：add/ls/rm/enable/disable | P2 |
| 2 | `paseo host ls` | List saved hosts, including disabled and | ❌ | 主机 + 在线状态 + 重连字段 | P2 |
| 3 | `paseo ls` | List agents. By default excludes archived | ⚠️ | 只列 run，不含 agent 概念/归档过滤；P3 改为 agent 维度 | P1 |
| 4 | `paseo run` | Create and start an agent with a task | ⚠️ | 已支持 prompt/agent/timeout/workspace；缺 provider 指定、会话续跑、worktree 隔离 | P1 |
| 5 | `paseo import` | Import an existing provider session as a | ❌ | 导入已有 provider 会话（依赖会话持久化） | P3 |
| 6 | `paseo clone` | Clone a GitHub repo and register it as a | ❌ | 克隆仓并注册为工作区 | P4 |
| 7 | `paseo attach` | Attach to a running agent's output stream | ❌ | 接入运行中 agent 输出流（SSE/长连接） | P3 |
| 8 | `paseo logs` | View agent activity/timeline | ✅ | `/v1/runs/{id}/log` 可用，200KB 尾部截断 | P1 |
| 9 | `paseo inspect` | Show detailed information about an agent | ❌ | agent 详情 | P3 |
| 10 | `paseo wait` | Wait for an agent to become idle | ❌ | 等待进入 idle | P3 |
| 11 | `paseo archive` | Archive an agent (soft-delete) | ❌ | 归档 agent（软删除） | P3 |
| 12 | `paseo onboard` | Run first-time setup, start daemon, and | ❌ | 首次引导 + 起 daemon + 打印配对信息 | P2 |
| 13 | `paseo start` | Start the local daemon from persistent | ⚠️ | `serve` 等价；缺持久化配置驱动的 `start` | P1 |
| 14 | `paseo hooks` | Record agent hook activity | ❌ | 记录 agent hook 事件 | P5 |
| 15 | `paseo status` | Observe the selected daemon and its | ⚠️ | `roamway status` 只看本机；缺 supervisor/endpoint 探测 | P1 |
| 16 | `paseo restart` | Restart the selected daemon worker, | ❌ | 需 supervisor（systemd/LaunchAgent/cron）配合 | P2 |
| 17 | `paseo reload` | Reload config.json without restarting | ❌ | 热加载 config 不重启 | P2 |
| 18 | `paseo pair` | Print the daemon pairing QR code and link | ❌ | 配对二维码/链接（对应 ECDH 配对） | P6 |
| 19 | `paseo agent` | Manage agents (advanced operations) | ❌ | agent 子命令族 | P3 |
| 20 | `paseo agent ls` | List agents. By default excludes archived | ❌ | 按 agent 维度列出（含归档） | P3 |
| 21 | `paseo agent run` | Create and start an agent with a task | ❌ | 创建 agent 并启动任务 | P3 |
| 22 | `paseo agent import` | Import an existing provider session as a Paseo | ❌ | 导入已有 provider 会话 | P3 |
| 23 | `paseo agent attach` | Attach to a running agent's output stream | ❌ | 接入运行中 agent 输出流 | P3 |
| 24 | `paseo agent logs` | View agent activity/timeline | ❌ | agent 时间线 | P3 |
| 25 | `paseo agent open` | Open an existing agent in Paseo Desktop | ❌ | 控制面内打开 agent | P10 |
| 26 | `paseo agent inspect` | Show detailed information about an agent | ❌ | 详情：provider/模型/会话/工作区/状态 | P3 |
| 27 | `paseo agent wait` | Wait for an agent to become idle | ❌ | 等待 agent 进入 idle | P3 |
| 28 | `paseo agent archive` | Archive an agent (soft-delete) | ❌ | 软删除 | P3 |
| 29 | `paseo agent reload` | Reload an agent (restarts the underlying | ❌ | 重启底层会话 | P3 |
| 30 | `paseo agent detach` | Make a subagent independent without stopping or | ❌ | subagent 转独立 agent 不中断 | P3 |
| 31 | `paseo agent update` | Update an agent's settings or metadata | ❌ | 更新设置/元数据 | P3 |
| 32 | `paseo daemon` | Manage the Paseo daemon | ❌ | daemon 子命令族 | P2 |
| 33 | `paseo daemon start` | Start the local daemon from persistent configuration | ❌ | 从配置拉起 | P2 |
| 34 | `paseo daemon run` | Run a local daemon in the foreground with deployment | ⚠️ | `serve` 前台运行已具备，缺部署参数集 | P2 |
| 35 | `paseo daemon status` | Observe the selected daemon and its published | ⚠️ | 本地健康有；缺已发布端点观测 | P2 |
| 36 | `paseo daemon stop` | Stop the selected daemon | ❌ | 优雅停止（SIGTERM + 子进程回收） | P2 |
| 37 | `paseo daemon restart` | Restart the selected daemon worker, retaining its | ❌ | 重启 worker，保留 supervisor 启动 | P2 |
| 38 | `paseo daemon reload` | Reload config.json without restarting | ❌ | 重载 config | P2 |
| 39 | `paseo daemon pair` | Print the daemon pairing QR code and link | ❌ | 配对信息输出 | P6 |
| 40 | `paseo daemon config` | Read or edit persistent local configuration (local | ❌ | 读写持久化配置 | P2 |
| 41 | `paseo daemon set-password` | Save a hashed daemon password (local operation) | ❌ | 密码哈希存储 | P6 |
| 42 | `paseo hub` | Manage Paseo Hub | ❌ | Hub 族 | P9 |
| 43 | `paseo hub init` | Create and optionally deploy a safe starter Hub | ❌ | starter Hub | P9 |
| 44 | `paseo hub permissions` | Manage this Hub's daemon permissions | ❌ | Hub 权限 | P9 |
| 45 | `paseo hub projects` | List projects for the authenticated Hub | ❌ | Hub 项目 | P9 |
| 46 | `paseo hub deploy` | Deploy .paseo organization triggers or a legacy | ❌ | 部署触发器 | P9 |
| 47 | `paseo hub logout` | Remove the active stored Hub CLI login | ❌ | 登出 | P9 |
| 48 | `paseo terminal` | Manage workspace terminals | ❌ | 工作区终端族 | P7 |
| 49 | `paseo terminal ls` | List terminals | ❌ | 列出终端 | P7 |
| 50 | `paseo terminal create` | Create a terminal | ❌ | 创建 pty | P7 |
| 51 | `paseo terminal kill` | Kill a terminal | ❌ | 关闭终端 | P7 |
| 52 | `paseo terminal capture` | Capture terminal output | ❌ | 抓取输出 | P7 |
| 53 | `paseo terminal send-keys` | Send keys to a terminal | ❌ | 发送按键 | P7 |
| 54 | `paseo script` | Manage configured workspace scripts | ❌ | 工作区脚本族 | P7 |
| 55 | `paseo script ls` | List configured workspace scripts | ❌ | 列出脚本 | P7 |
| 56 | `paseo script start` | Start a configured workspace script | ❌ | 启动常驻脚本 | P7 |
| 57 | `paseo script stop` | Stop a running workspace script | ❌ | 停止脚本 | P7 |
| 58 | `paseo schedule` | Manage recurring schedules | ⚠️ | 现有 jobs 仅 every_secs/daily | P5 |
| 59 | `paseo schedule create` | Create a schedule | ⚠️ | 缺 cron、IANA 时区、max-runs、expires-in、--run-now | P5 |
| 60 | `paseo schedule ls` | List schedules | ⚠️ | `jobs` 可用 | P5 |
| 61 | `paseo schedule inspect` | Inspect a schedule | ❌ | 单个 schedule 详情 | P5 |
| 62 | `paseo schedule logs` | Show recent schedule run logs | ❌ | schedule 维度日志 | P5 |
| 63 | `paseo schedule pause` | Pause a schedule | ❌ | 暂停 | P5 |
| 64 | `paseo schedule resume` | Resume a paused schedule | ❌ | 恢复 | P5 |
| 65 | `paseo schedule delete` | Delete a schedule | ⚠️ | `jobs-rm` 有；缺 purge_runs | P5 |
| 66 | `paseo schedule run-once` | Manually trigger a single run of a schedule | ❌ | 手动触发一次 | P5 |
| 67 | `paseo schedule update` | Update an existing schedule in place | ❌ | 原地更新 | P5 |
| 68 | `paseo heartbeat` | Manage this agent's heartbeats | ❌ | agent 心跳族 | P5 |
| 69 | `paseo heartbeat create` | Create a recurring prompt for this agent | ❌ | 周期性 prompt（复用同一会话） | P5 |
| 70 | `paseo heartbeat update` | Change a heartbeat cron cadence | ❌ | 改 cron 频率 | P5 |
| 71 | `paseo heartbeat delete` | Delete a heartbeat | ❌ | 删除心跳 | P5 |
| 72 | `paseo permit` | Manage permission requests | ❌ | 权限请求族 | P6 |
| 73 | `paseo permit ls` | List all pending permissions | ❌ | 待审批请求列表 | P6 |
| 74 | `paseo provider` | Manage agent providers | ⚠️ | 4 agent + 2 api，无统一抽象 | P8 |
| 75 | `paseo provider ls` | List available providers and status | ❌ | provider 列表与状态 | P8 |
| 76 | `paseo provider models` | List models for a provider | ❌ | 模型列表 | P8 |
| 77 | `paseo provider diagnostic` | Show provider installation, environment, and | ❌ | 安装/环境/PATH 诊断 | P8 |
| 78 | `paseo usage` | Show account usage reports | ❌ | 用量报表 | P8 |
| 79 | `paseo usage ls` | List account quotas and balances | ❌ | 额度与余额 | P8 |
| 80 | `paseo usage inspect` | Show a complete account usage report | ❌ | 完整报告 | P8 |
| 81 | `paseo plugin` | Manage trusted, unsandboxed plugins | ❌ | 插件族 | P8 |
| 82 | `paseo plugin init` | Create a typecheckable local plugin | ❌ | 插件骨架 | P8 |
| 83 | `paseo plugin logs` | Show recent plugin output | ❌ | 插件日志 | P8 |
| 84 | `paseo plugin reload` | reload a plugin | ❌ | 重载 | P8 |
| 85 | `paseo plugin enable` | enable a plugin | ❌ | 启用 | P8 |
| 86 | `paseo plugin disable` | disable a plugin | ❌ | 禁用 | P8 |
| 87 | `paseo plugin remove` | Remove plugin configuration | ❌ | 移除 | P8 |
| 88 | `paseo speech` | Speech commands | ❌ | 语音命令 | P8 |
| 89 | `paseo project` | Manage projects | ❌ | 项目子命令族 | P4 |
| 90 | `paseo project ls` | List projects | ❌ | 列出项目 | P4 |
| 91 | `paseo project delete` | Delete a project and its workspaces | ❌ | 删除项目及其工作区 | P4 |
| 92 | `paseo workspace` | Manage workspaces | ❌ | workspace 子命令族 | P4 |
| 93 | `paseo workspace setup` | Allow and run setup for a workspace | ❌ | 首次运行 setup 脚本 | P4 |
| 94 | `paseo workspace create` | Create a workspace | ❌ | 创建工作区（含 git worktree） | P4 |
| 95 | `paseo workspace ls` | List active workspaces | ❌ | 列出活跃工作区 | P4 |
| 96 | `paseo workspace archive` | Archive a workspace and everything it owns | ❌ | 归档工作区及其 agent | P4 |

## 二、非 CLI 能力（Paseo 有、必须对齐）

| 能力 | Paseo 现状 | roamway 现状 | 目标 | 阶段 |
|---|---|---|---|---|
| 控制面 UI | Web + 桌面（Electron）+ 手机 | ❌ 无 | 轻量 Web（浏览器/手机浏览器），不做 Electron 同构 | P10 |
| Agent 时间线 | `logs` / `inspect` / attach 流 | ⚠️ 只有纯文本日志 | 结构化事件时间线（tool call / 审批 / 错误） | P3 |
| Diff 视图 | UI 内看改动 | ❌ | Web 内 git diff | P10 |
| 审批流 | `permit allow/deny`，UI 弹窗 | ❌ | 风险命令中断待批 + API 审批 | P6 |
| Diff/终端/浏览器驱动 | UI 内置 | ❌ | 终端 P7，diff/浏览器 P10 | P7/P10 |
| 会话续跑 | `sessionId` + `nativeHandle` | ❌ | 记录 provider 会话 id，可 resume | P3 |
| Worktree 隔离 | 每 agent 独立 worktree | ❌ | `workspace create` 建 worktree 并绑定 agent | P4 |
| 配对加密 | ECDH + XSalsa20-Poly1305 | ❌ 单 token 明文 | 复用同一协议或等价的 X25519 + ChaCha20-Poly1305 | P6 |
| 移动端 | 官方手机 App | ❌ | 移动浏览器访问 P10 的 Web | P10 |
| Hub 事件触发 | GitHub/Slack/Discord/Linear | ❌ | Webhook 接收 + 触发 agent | P9 |
| 插件系统 | 可信非沙箱插件 + TS SDK | ❌ | Rust 动态加载 或 子进程协议（优先子进程，安全边界清楚） | P8 |
| 多机路由 | `host` + `serverId` 作用域 | ❌ 只能 `--host` 指一台 | 注册表 + 健康探测 + 跨机派活 | P2 |

## 三、阶段排期（按依赖顺序，不做并行跳跃）

| 阶段 | 内容 | 命令数 | 依赖 | 交付判据 |
|---|---|---|---|---|
| P1 | 执行底座（正确性，codex 进行中） | 5 | — | NAS 9 条验收全过 + CI 三平台绿 |
| P2 | 主机联邦与守护进程 | 13 | P1 | 三台 host 注册、跨机派活成功、断线重连 |
| P3 | Agent 生命周期与会话 | 17 | P2（跨机 agent 必须先有 host 路由） | 同一 agent 可续跑、可 attach 流、可归档 |
| P4 | 工作区、项目与并行隔离 | 9 | P3 | 两 agent 并行不互相踩（独立 worktree） |
| P5 | 调度、心跳与钩子 | 15 | P3（heartbeat 需要会话） | cron/时区/max-runs/pause 全套 + 日志 |
| P6 | 权限与安全 | 5 | P2 | 无 token 拒绝启动、审批流可拦下命令、配对可用 |
| P7 | 终端与工作区脚本 | 10 | P4（终端属于 workspace） | Web 内开终端并回放输出 |
| P8 | 提供商、用量、插件与语音 | 15 | P3 | provider ls/models/diagnostic 可用 |
| P9 | Hub 与事件触发 | 6 | P3/P8 | GitHub webhook 触发一次 agent |
| P10 | 控制面（Web UI） | 1 | P3/P4 | 浏览器里看 agent 列表/时间线/diff 并操作 |

## 四、验收方式（每条命令）

1. `roamway <cmd> --help` 的参数集与 Paseo 同名命令对齐（允许新增，不允许缺必选参数）。
2. 输出支持 `--json`（与 Paseo `-o json` 等价），字段名尽量沿用 Paseo，便于脚本迁移。
3. 行为对齐：同一动作在三端（PC / NAS / Mac）实测通过，输出贴进本条证据列。
4. 对齐不了的，必须在表里写明"不做"的理由，不允许静默跳过。

## 五、当前整体完成度

- CLI：**96 条中 ✅1 / ⚠️11 / ❌84**，即约 **1% 完全对齐、12.5% 可用**。
- 非 CLI：12 项里 **0 项对齐**，2 项部分（日志、调度语义）。
- 结论：**作为 Paseo 的替代品，目前不成立**；P1 完成（正确性）后从 P2 开始按阶段推进。
