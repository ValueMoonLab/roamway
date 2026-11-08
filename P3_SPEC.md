# P3 规范：Agent 生命周期与会话（PARITY P3，17 条命令）

目标：把「一次性 run」升级为「可复用的 agent」——有身份、有会话、能续跑、能 attach、能归档。Paseo 里 `agent` 那 12 个子命令和 `import/attach/inspect/wait/archive` 都要在这儿落地。

前置：P1（进程组/并发/持久化）、P2（host 注册表、dispatch）。全部新命令都支持 `--host`。

## 1. 数据模型（`src/agents.rs` 新增，持久化进 state.json）

```rust
pub struct Agent {
    pub id: String,            // ag_ + 8 hex
    pub title: String,         // 默认取首条 prompt 前 60 字符
    pub kind: String,          // shell | codex | claude | api:deepseek | api:qwen
    pub model: String,
    pub workspace: String,
    pub session_id: String,    // provider 侧会话 id；api 类型为空
    pub state: String,         // idle | running | archived | error
    pub parent: String,        // 子 agent 的父 id，空表示顶层
    pub history: Vec<Msg>,     // api 类型用于多轮重放；其他类型只留最近 20 条
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_id: String,
    pub last_error: String,
}
pub struct Msg { pub role: String, pub content: String }  // role: user | assistant
```

- `state.json` 加 `agents: Vec<Agent>`，`#[serde(default)]` 保证旧文件可读。
- run 与 agent 的关系：`Run.agent_id`（新增，`#[serde(default)]`）。没有 agent_id 的旧 run 照常工作。

## 2. 命令表

| 命令 | 行为 |
|---|---|
| `agent run <prompt> [--title --workspace --model --kind --no-wait]` | 建 agent + 起首个 run，输出 agent 与 run |
| `agent ls [--all] [--json]` | 默认不含 archived；表格 ID STATE KIND MODEL TITLE UPDATED |
| `agent inspect <id> [--json]` | 全字段 + 该 agent 最近 5 次 run 摘要 |
| `agent logs <id> [--tail N]` | 汇总该 agent 所有 run 的日志尾部 |
| `agent wait <id> [--timeout S]` | 轮询到 state != running，超时返回非 0 |
| `agent attach <id>` | 流式输出（见 §4） |
| `agent archive <id>` | state=archived，不删数据 |
| `agent reload <id>` | 会话重置：清 session_id/history，用同一 title/workspace 起新 run |
| `agent update <id> [--title --model --workspace]` | 改元数据 |
| `agent open <id>` | 打印可控 URL（P10 之前打印 `http://<bind>/agents/<id>`） |
| `agent detach <id>` | 把 parent 置空（子 agent 转独立）；无 parent 时提示 already independent |
| `agent import --kind codex --session <sid> [--title]` | 用已有 provider 会话建 agent（session_id 落库，state=idle） |
| `import <id> --session <sid>` | 同上的顶层别名 |
| `attach <id>` / `inspect <id>` / `wait <id>` / `archive <id>` | 顶层别名，语义一致 |

## 3. 多轮与续跑

- `api:*` 类型：每次新 prompt 时，把 `history` 拼进 `messages`（保留 system + 最近 20 轮），返回后写回。
- `codex` 类型：首轮成功后从 codex 输出里解析 `session id:`（`codex exec` 会打印），存 `session_id`；后续轮用 `codex exec resume <sid> "<prompt>"`；解析不到就退化为每次新会话，并在 `last_error` 里注明 `resume unsupported`。
- `claude` 类型：`claude -p --session-id <uuid>` 或 `--resume`（实现时以本机 `claude --help` 为准，探测不到就退化）。
- `shell` 类型：无会话概念，每轮独立，`history` 仍记录 prompt 方便 logs。

## 4. `attach` 流式

- 新增 `GET /v1/agents/{id}/events`：`Content-Type: text/event-stream`，事件 `{"type":"log","chunk":"..."}` / `{"type":"state","state":"idle"}`，每 15s 发心跳注释 `: ping`，agent 结束即关闭。
- CLI `attach`：逐块写到 stdout，结束时打印 `state=<x>`；`Ctrl-C` 只断开不断任务。
- 未实现的事件类型不得发送。

## 5. HTTP 路由（新增）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/v1/agents` | 列表，`?all=1` 含归档 |
| POST | `/v1/agents` | 创建（等价 `agent run`） |
| GET | `/v1/agents/{id}` | 详情 |
| PATCH | `/v1/agents/{id}` | 改 title/model/workspace |
| DELETE | `/v1/agents/{id}` | 归档 |
| GET | `/v1/agents/{id}/events` | SSE |
| POST | `/v1/agents/{id}/reload` | 会话重置 |
| POST | `/v1/agents/{id}/detach` | 断开 parent |

全部走既有鉴权；`/v1/agents/{id}/events` 允许 `?token=` 查询参数（SSE 无法带 header 的客户端用），其余不接受查询参数 token。

## 6. 单测

- `agent_id_unique`
- `title_from_prompt_truncates_60`
- `state_transitions_idle_running_idle`
- `archive_hides_from_default_list`
- `history_capped_at_20`
- `session_id_recorded_only_for_resumable_kinds`
- `agent_roundtrip_in_state_json`（含旧 state.json 无 agents 字段时能加载）
- `detach_clears_parent`

## 7. 验收

1. `agent run "echo hi"` → `agent ls` 看到 idle；`agent inspect` 字段齐
2. `agent run "sleep 5"` → 立刻 `agent wait` 阻塞到 idle，退出码 0
3. `agent attach <id>` 能实时看到输出，结束后打印 `state=idle`
4. `api:deepseek` 两轮对话：第二轮能引用第一轮内容（history 生效）
5. `codex` 两轮：第二轮日志出现 `resume`（或明确记录 unsupported）
6. `agent archive` 后 `agent ls` 不显示，`--all` 显示
7. `agent reload` 后 session_id 清空、新 run 成功
8. 旧 `state.json`（无 agents 字段）能被新版本加载，已有 run 不丢
9. `cargo build --release` 0 warning，`cargo test` 全过，CI 三平台绿

## 8. 约束

- 不改 P1/P2 既有行为；`run` 仍是无状态一次性执行入口（=`agent run` 的底层）。
- 所有新命令支持 `--host` 与 `--json`。
- Windows/macOS/Linux 三平台可编译；进程回收复用 P1 的 `kill_tree`。
