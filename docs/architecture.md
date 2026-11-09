# 设计取舍

这份文档解释 roamway 为什么长成这样，方便你判断该不该用、以及改的时候会不会踩坑。

## 目标与非目标

**目标**：在无人值守的机器上，可靠地、有界地、可观测地跑一批 agent 任务。

**非目标**：交互式体验、多人协作、实时看板、跨机联邦。这些交给 Paseo、CI、或别的工具。

一句话检验标准：*daemon 被 kill -9 之后重启，系统应该处于可解释状态，而不是"任务跑了一半又重来一遍"。*

## 为什么是线程 + 阻塞 IO

单机并发上限是"同时跑几个 agent"，现实中 1–20 个。这个量级下 tokio 的复杂度换不来收益，反而让"子进程生命周期 + 取消 + 超时"三件事写得更绕。所以：

- 每连接一线程（`std::net::TcpListener` + `thread::spawn`），带连接数上限，超过直接 `503`；
- 每个 run 一个线程（`runner::start`），用 `Child::try_wait` 轮询等待，轮询窗口兼做取消与超时检查；
- run 并发数由 `max_concurrent` 限制，超出部分保持 `queued` 排队；
- 共享状态一把 `Mutex<Store>`，只在读写状态时短暂持有，不在锁内做 IO。

代价：连接数上千会退化。对"一个人 + 十几台 agent"完全够用。

## 状态持久化

`~/.roamway/state.json`，每次状态变更后整份写盘：

1. 写入 `state.json.tmp` 并 `File::sync_all()` 刷盘；
2. 若旧 `state.json` 存在，`rename` 成 `state.json.bak`（保留最近一份可用副本）；
3. `rename` 覆盖正式文件（同一文件系统上原子）；
4. Unix 上再对父目录 `sync_all()`，确保 rename 本身也落盘。

启动恢复顺序：先读 `state.json`；空文件视为空状态；解析失败时把损坏文件 `rename` 成 `state.json.corrupt-<ts>`（保留现场），再尝试 `state.json.bak`；都失败才用空状态并打印错误。只有 load 成功（含从 bak 恢复）才会在启动时立即 save，避免断电半写把 jobs 清空。

不引数据库的理由：数据量是几百条 run，全量写盘 < 5 ms，省掉部署依赖和迁移问题。

## 子进程模型

```
runner thread ──spawn──▶ codex/hermes/claude/sh (新进程组)
      │                        │
      │◀────── stdout+stderr ──┘  (重定向到 runs/<id>.log)
      │
      ├── 等待退出 / 超时 / 被 stop
      └── 写回 done|failed|timeout|killed + exit code
```

- 日志直接重定向到文件，不经过内存缓冲，长任务不会吃爆内存。
- Unix 上 spawn 前设 `process_group(0)`，回收统一走 `kill_tree`：先 SIGTERM，2 秒后对整个进程组 SIGKILL，确保 `sh -c "sleep 30"` 的孙进程也被带走。
- Windows 上用 `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`，回收走 `taskkill /T /F`，不弹黑窗。
- `timeout_s` 到点走同一套 `kill_tree`，标 `timeout`。
- `stop` 不直接操作进程：先在 `cancels: HashMap<id, Arc<AtomicBool>>` 里写取消标志，再改状态为 `killed`；runner 每 300ms 检查一次，命中就 `kill_tree`。
- 子进程退出码 0 → `done`，非 0 → `failed`，同时记录 stderr 尾部。

## 并发与排队

- 连接线程上限 `--max-conn`（默认 64）：达到上限立即回 `503` 并关闭连接，accept 循环不阻塞。
- run 并发上限 `--max-concurrent`（默认 4）：新建 run 一律先落为 `queued`；低于上限时立即启动，否则由调度器每轮按空闲槽位拾取。
- 手动 run 与 job 触发的 run 走同一条排队路径（`drain_queued`），保证"同时提交 6 个只跑 2 个"。
- `/health`（免鉴权）与 `/v1/health`（需鉴权）返回 `running` / `queued` 计数。

## 重启语义

启动时扫一遍 state：凡是 `queued` / `running` 的 run 一律改判 `interrupted`，绝不自动重放。若 run 记录了 pid，先用 `pid_is_our_run` 校验（Linux 对比 `/proc/<pid>/stat` 的 starttime 与记录启动时间，容差 30s），确认仍属于本次 run 才调用 `kill_tree` 回收进程组，降低 PID 复用误杀。

这是刻意的选择。自动重跑看起来更"智能"，但对会写库、会发请求、会花钱的任务来说，"重跑一半"比"停下来等人看"危险得多。需要重试的场景，交给 job 的退避机制——它有次数上限。

## 调度器

每秒扫一遍所有 job：

```
if !enabled { skip }
if next_at > now { skip }
if 同 job 有未终态 run && !allow_overlap { skip }   // 这里不推进 next_at，避免漂移
run 结束后: 成功 → next_at = now + every_secs
            失败 → next_at = now + backoff(attempts)
            attempts > max_retries → enabled = false
```

`backoff(n) = min(30 * 2^(n-1), 3600)`。上限一小时的用意：临时故障（网络抖动、模型 429）能自愈；持续性故障（key 失效、脚本报错）不会变成无限刷。

## 有界性

长期运行最容易被忽视的是"什么都不删"：

| 资源 | 上限 |
|---|---|
| run 记录 | 最近 800 条 |
| 连接线程 | `max_conn`，默认 64 |
| 并发 run | `max_concurrent`，默认 4 |
| 日志文件 | 随记录一起删；单次读取 200 KB 封顶 |
| 单 run 时长 | `timeout_s`，默认 1800s |
| 重试次数 | `max_retries`，默认 2（设为 0 表示不重试） |
| 重试间隔 | 1 小时 |
| HTTP 请求体 | 1 MB |

## 安全模型

- 默认绑 `127.0.0.1`。放开到 `0.0.0.0` 是显式动作，未配置 token 且绑定非 loopback 时拒绝启动（退出码 2）。
- 鉴权三种入口：多 token（`<data>/tokens.json`，存 sha256、scope、`expires_at`）、密码登录（PBKDF2-HMAC-SHA256，`password.json`，`POST /v1/auth/login` 换 24h 短期 token）、一次性配对码（`pair` → `POST /v1/auth/pair` 换长期 token）。`--token`/`ROAMWAY_TOKEN` 仍是单 token 兼容入口。
- scope 三层：`read`（GET/HEAD/OPTIONS）、`run`（建 run/agent）、`admin`（hosts/schedules/daemon/stop 等）；`admin` 包含全部；scope 不足返回 `403`。
- 所有响应带 `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY`、`Referrer-Policy: no-referrer`；Host 白名单不匹配返回 `421`，CORS 仅在配置 `--allow-origin` 时允许并处理 OPTIONS 预检。
- 审批流：`--require-approval run|admin` 命中写请求时先落 `approvals.json`（记录 method/path/body_hash/token_id/status），返回 `202 + approval_id`；`approve` 后客户端以 `X-Approval` 重放，body hash 一致才执行，不一致返回 `409`；记录保留 7 天。
- 敏感文件（`tokens.json`/`password.json`/`approvals.json`）0600 落盘（Unix）；密码只存 PBKDF2 salt/hash/iterations，token 只存 sha256，日志不输出 token/密码/内网 IP/宿主绝对路径。
- 不做完整 RBAC 与 Web 用户体系：这是单用户工具，需要多用户请放反代后面。`--tls-cert`/`--tls-key` 当前为占位，TLS 直连未实现，公网请走 TLS 反代。
- agent 子进程以 daemon 用户身份运行。要隔离就用容器或独立系统用户，别指望 roamway 做沙箱。
- agent 采用白名单（`shell`/`codex`/`hermes`/`claude`/`api:deepseek`/`api:qwen`），其余一律 400，避免把字段直接当可执行文件名。
- `codex` 的 `workspace-write` 是 codex 自己的能力，不是 roamway 的；`shell` agent 没有任何沙箱。

## 可移植性

- 子进程回收按平台分叉：Unix 用 `libc`（`[target.'cfg(unix)'.dependencies]`，仅编入 Unix）做进程组 `kill`；Windows 用 `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` + `taskkill /T /F`。
- 其余平台差异极小：`std::process::Command` 在 Windows 上是 `CreateProcess`，`sh -c` 自动切成 `cmd /C`。
- 路径全部来自参数或环境变量，没有硬编码目录。
- 数据目录默认 `$HOME/.roamway`（Windows 用 `%USERPROFILE%`）。
- 时间统一用 Unix 秒，`daily` 的时区偏移可配，不依赖系统 TZ 数据库。
- macOS 上 Apple Silicon 用 `aarch64-apple-darwin`、Intel 用 `x86_64-apple-darwin`；国内可用 rsproxy.cn 镜像。
- CI 在 Linux / macOS / Windows 三平台编译并跑测试。
