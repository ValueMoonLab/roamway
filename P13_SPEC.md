# P13_SPEC — provider 会话续跑（run.resumed / session API / run --resume）

前置事实（实测于 P12 之后的 HEAD，锚点为文件:函数）：

- `src/agents.rs:71 fn is_resumable_kind(kind)` → 只有 `codex` / `claude` 有 provider 侧会话；
  `shell`、`api:*` 没有可 resume 的 session id。`Agent.session_id` 是唯一会话载体（`agents.rs:16`）。
- claude 路径（`src/runner.rs:96-163`）：`probe_claude_flags()` 决定用 `--session-id` 还是 `--resume`；
  probe 失败或拿不到 flag 时 `resume_note = Some("resume unsupported")`。
- codex 路径（同文件）：run 结束后 `extract_codex_session(&log_path)` 从日志提取 sid，
  成功则 `record_agent_session`，失败同样置 `resume_note`。
- `resume_note` 唯一的落点是 `mark_agent_error(&app, &run.agent_id, &note)`，即写进 `agent.last_error`；
  **`Run` 上没有任何字段体现**（`src/types.rs:119` 无 `resumed` / `resume_note`），
  `run` 的 CLI 输出也不会告诉调用方"这次是不是续跑、有没有续上"。
- `POST /v1/agents/{id}/run`（`server.rs:1734 fn continue_agent_route` → `agents::queue_agent_run`）已能续跑，
  但**没有只读端点**能查 agent 的会话状态；`GET /v1/agents/{id}`（`server.rs:1615`）会带 `session_id`，
  却混在完整 agent 对象里，且没有 `resumable` 判定。
- `cmd_agent_inspect`（`src/main.rs:2948`）只打印 id/title/kind/model/state/workspace 等，不显示 session/resumable。

本阶段目标：把"会话续跑"从隐式行为变成**可查询、可断言、可测试**的显式契约。不改 `POST /v1/runs`，
不改 P1–P3 的既有 200/4xx 形状；只做新增字段与新增只读端点（新字段一律 serde default，旧 state.json 可读）。

---

## 一、数据模型（src/types.rs）

1. `Run` 新增：
   - `resumed: bool`，`#[serde(default)]`，**总是序列化**（false 也要出现，客户端可无条件读取）
   - `resume_note: Option<String>`，`#[serde(default, skip_serializing_if = "Option::is_none")]`
2. 语义：
   - `resumed = true` 当且仅当 run 在派发时使用了 provider 会话，即
     `is_resumable_kind(agent.kind) && !agent.session_id.trim().is_empty()`（首个 run 通常 false）
   - `resume_note` 只在"本该续跑但没续上"时出现：值固定 `"resume unsupported"`（沿用现有措辞，别造新词）
3. 所有 `Run { .. }` 构造点补两个字段（P12 已因 `labels` 踩过一遍，同样清单：
   `create_run`、`agents::create_agent_and_run`、`agents::queue_agent_run`、`agents::reload_agent`、
   `schedules.rs` 的触发路径）。`agents::queue_agent_run` 计算 `resumed`，其余传 `false`。

## 二、runner 行为（src/runner.rs）

1. `run_agent` 在派发前计算 `resumed` 并写进 run（在 `app.store` 锁内更新，不要并发写整个 state）。
2. `resume_note` 除现有 `mark_agent_error` 外，**同时**写到 run：
   - `run.resume_note = Some(note)`
   - 当 `status == Done` 时 `run.error = Some(note)`（status 保持 Done，`exit_code` 保持 0，不得改成 Failed）
   - 至少一处日志行：`[resume] unsupported kind=… agent=…`
3. 不得因为 `resume_note` 触发插件事件变化（`agent.finished` 仍按 status 发）。

## 三、只读会话端点（src/server.rs）

1. 新增 `GET /v1/agents/{id}/session`：
   ```json
   {
     "agent_id": "ag_…",
     "kind": "codex",
     "resumable": true,
     "has_session": true,
     "session_id": "…",
     "state": "idle",
     "last_error": "",
     "last_run_id": "run_…"
   }
   ```
   - `resumable` = `is_resumable_kind(kind)`；`has_session` = `!session_id.trim().is_empty()`（与 resumable 独立）
   - 无此 agent → 404 `{"error": "no such agent"}`；认证与其他 `/v1/agents/*` 一致（走现有 route 的 auth 前置）
2. 路由必须加在 `["v1","agents",id]` 的 GET 分发**之前**（`server.rs:1086` 附近已有 `diff`/`events` 特例，照抄顺序写法），
   否则会被 `get_agent_route` 吞掉——这是本阶段最容易踩的坑，用测试兜底（见四.3）。
3. `GET /v1/agents/{id}` 保持原样（不删 `session_id` 字段），额外加 `"resumable": bool`（顶层，与 `agent` 平级），
   不动 `agent` 对象内部结构。

## 四、CLI（src/main.rs）

1. `run --resume <agent_id> [prompt]`：等价 `POST /v1/agents/{id}/run`（`{prompt, timeout_s}`），
   输出契约与 `run` 一致（首行 run id；`--no-wait` 只打 run id；等待时轮询 `/v1/runs/{id}`）。
   `run --continue <agent_id>` 是同一实现的别名（`--continue` 与 `--resume` 同时给 → exit 2）。
   `--resume` 与位置参数 prompt 同时给时，prompt 取位置参数；都不给 → exit 2 `prompt required`。
2. `agent inspect <id>` 增加三行：`resumable: yes|no`、`session: <id 或 ->`、`last_error: <…>`（空则 `-`）。
3. `agent ls` 的 JSON 输出不改；表格式不加列（避免破坏脚本 grep），resume 信息只在 inspect/session 里。
4. 未知 agent id：服务端 404 → CLI stderr `resume failed: no such agent` + exit 1（沿用网络失败码）。

## 五、单测

1. `fn should_resume(kind: &str, session_id: &str) -> bool`：`("codex","")=false`、`("codex","x")=true`、
   `("shell","x")=false`、`("api:deepseek","")=false`。
2. `fn resume_target(flags, pos) -> Result<(String, String), String>`：`--resume ag_1 "do"` → `(ag_1, do)`；
   `--resume` 无 prompt → Err；`--resume` + `--continue` → Err。
3. 路由顺序：`fn route_session_path(path) -> &'static str` 之类的纯函数，或直接对 `route()` 做单元测试
   （现有 `#[cfg(test)]` 里已有构造 `Req` 的辅助，可复用），断言 `GET v1/agents/ag_x/session` 命中 session 分支而非
   `get_agent_route`（返回值含 `"resumable"` 键即可判定）。
4. `resume_note` 落点：构造一个 Done 且 note 存在的场景（可直接调纯函数 `apply_resume_note(&mut run, note)`），
   断言 status 仍 Done、`error == Some("resume unsupported")`、`resume_note` 同值。

## 六、文档

- `docs/providers.md`：新增「会话续跑」小节——codex 从日志提取 session id、claude 靠 `--session-id/--resume` probe、
  `shell`/`api:*` 明确不支持；给出 `run --resume` 与 `GET /v1/agents/{id}/session` 用法。
- `PARITY.md`：把"provider 会话续跑"独立成行，状态 ⚠️（首轮续跑 ✅、unsupported 显式化 ✅、
  `--thinking` 等未对齐项仍在），证据写函数名与测试名。

## 七、验收（keeper 实测，NAS 8796，用真实 codex/claude 不现实则用 shell 走 unsupported 分支）

- NAS `cargo build --release && cargo test` 全绿、0 warning。
- `curl -s -H "Authorization: Bearer $T" $B/v1/agents/<id>/session` → 含 `resumable` 与 `has_session`；
  `<id>` 不存在 → 404。
- `roamway run --resume ag_nope hi` → rc=1，stderr 含 `no such agent`。
- 用一个 shell agent 续跑一次：`resume_note` 出现在 `GET /v1/runs/<run_id>` 的 JSON 里，且 status 仍是 done。
- `roamway agent inspect <id>` 输出包含 `resumable:` 行。
