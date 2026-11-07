# 设计取舍

这份文档解释 roamway 为什么长成这样，方便你判断该不该用、以及改的时候会不会踩坑。

## 目标与非目标

**目标**：在无人值守的机器上，可靠地、有界地、可观测地跑一批 agent 任务。

**非目标**：交互式体验、多人协作、实时看板、跨机联邦。这些交给 Paseo、CI、或别的工具。

一句话检验标准：*daemon 被 kill -9 之后重启，系统应该处于可解释状态，而不是"任务跑了一半又重来一遍"。*

## 为什么是线程 + 阻塞 IO

单机并发上限是"同时跑几个 agent"，现实中 1–20 个。这个量级下 tokio 的复杂度换不来收益，反而让"子进程生命周期 + 取消 + 超时"三件事写得更绕。所以：

- 每连接一线程（`std::net::TcpListener` + `thread::spawn`）；
- 每个 run 一个线程（`runner::start`），用 `Child::wait` 阻塞等待；
- 共享状态一把 `Mutex<Store>`，只在读写状态时短暂持有，不在锁内做 IO。

代价：连接数上千会退化。对"一个人 + 十几台 agent"完全够用。

## 状态持久化

`~/.roamway/state.json`，每次状态变更后整份写盘：

1. 写到 `state.json.tmp`；
2. `rename` 覆盖正式文件（同一文件系统上原子）；
3. 进程崩溃最多丢掉"最后一次变更"，不会出现半个 JSON。

不引数据库的理由：数据量是几百条 run，全量写盘 < 5 ms，省掉部署依赖和迁移问题。

## 子进程模型

```
runner thread ──spawn──▶ codex/hermes/claude/sh
      │                        │
      │◀────── stdout+stderr ──┘  (重定向到 runs/<id>.log)
      │
      ├── 等待退出 / 超时 / 被 stop
      └── 写回 done|failed|timeout|killed + exit code
```

- 日志直接重定向到文件，不经过内存缓冲，长任务不会吃爆内存。
- `timeout_s` 到点 `kill`（Windows `taskkill /F`），标 `timeout`。
- `stop` 通过 `Mutex<HashMap<id, Child>>` 找到进程并 kill，标 `killed`。
- 子进程退出码 0 → `done`，非 0 → `failed`，同时记录 stderr 尾部。

## 重启语义

启动时扫一遍 state：凡是 `queued` / `running` 的 run 一律改判 `interrupted`。

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
| 日志文件 | 随记录一起删；单次读取 200 KB 封顶 |
| 单 run 时长 | `timeout_s`，默认 1800s |
| 重试次数 | `max_retries`，默认 0（不额外重试） |
| 重试间隔 | 1 小时 |
| HTTP 请求体 | 1 MB |

## 安全模型

- 默认绑 `127.0.0.1`。放开到 `0.0.0.0` 是显式动作。
- 单 token，比较用常量时间函数，避免计时侧信道。
- 不做 RBAC、不做用户体系：这是单用户工具，多用户请放反代后面。
- agent 子进程以 daemon 用户身份运行。要隔离就用容器或独立系统用户，别指望 roamway 做沙箱。
- `codex` 的 `workspace-write` 是 codex 自己的能力，不是 roamway 的；`shell` agent 没有任何沙箱。

## 可移植性

- 无平台相关依赖：`std::process::Command` 在 Windows 上是 `CreateProcess`，`sh -c` 自动切成 `cmd /C`。
- 路径全部来自参数或环境变量，没有硬编码目录。
- 数据目录默认 `$HOME/.roamway`（Windows 用 `%USERPROFILE%`）。
- 时间统一用 Unix 秒，`daily` 的时区偏移可配，不依赖系统 TZ 数据库。
- CI 在 Linux / macOS / Windows 三平台编译并跑测试。
