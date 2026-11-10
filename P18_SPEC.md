# P18_SPEC — `/metrics`（Prometheus 文本格式）

前置事实（实测于 P16 之前的 HEAD `eedfe2f`）：

- HTTP 入口 `src/server.rs:373 fn handle<C: Conn>(app: Arc<App>, mut s: C)`；静态 UI 在 `:417` 优先返回；
  auth 判定在 `:460-500` 区段（`needs_auth` `:682`，scope 检查走 `auth::required_scope`）；
  路由表 `src/server.rs:1023 fn route(app: &Arc<App>, req: &Req) -> (u16, Value)`，返回 **JSON Value**。
- 需要返回纯文本时用现成的 `:349 fn respond_static<C: Conn>(s, code, content_type, body: &[u8], extra: &[(&str, String)])`
  （UI 资源就是这么发的，`:424`）。
- `App` 定义在 `src/types.rs:372`，已有原子字段 `conns: AtomicUsize` 的先例；
  `App { … }` 构造点共 4 处：`src/main.rs:1148`、`src/main.rs:4770`、`src/server.rs:3091`、`src/server.rs:3178`
  —— 新增字段必须同时改这 4 处（`cargo check --all-targets` 会兜底）。
- 状态源：`app.store.lock()?.state`（`State { runs, jobs, agents, seq }`，`src/types.rs:215`）；
  `RunStatus` 枚举 `src/types.rs:91` 共 7 个变体：Queued/Running/Done/Failed/Timeout/Killed/Interrupted；
  调度 `app.schedules.lock()?.schedules`（`Schedule { enabled, paused, consecutive_failures }`）；
  终端 `app.terms.lock()`。
- `status_route`（`src/server.rs:1446`）演示了正确持锁姿势：`lock().unwrap_or_else(|e| e.into_inner())`，临界区只读、不跨长任务。

本阶段目标：`GET /metrics` 输出合法 Prometheus 文本，覆盖运行/队列/agent/job/调度/连接与 HTTP 计数。

---

## 一、端点语义

1. `GET /metrics` → 200，`Content-Type: text/plain; version=0.0.4; charset=utf-8`，body 为 expo 文本。
2. **鉴权沿用现有规则**：不加进 `needs_auth` 豁免列表；`Authorization: Bearer <token>` 与 `?token=<token>` 都要能过
   （与其它 `/v1/*` 一致）。无 token 或错 token → 与其它受保护路径相同的 401 响应。
3. 方法不是 GET → 405 `{"error":"method not allowed"}`（与既有非 GET 处理风格一致）。
4. 其余任何路径行为不变；`/health` 仍是唯一完全免鉴权的健康检查。

## 二、实现位置

1. `src/server.rs` 在 `handle()` 里、auth/scope 通过之后、`route(&app, &req)` 之前插入：
   ```rust
   if req.method == "GET" && req.path == "/metrics" {
       let body = metrics_text(&app);
       return respond_static(&mut s, 200, "text/plain; version=0.0.4; charset=utf-8", body.as_bytes(), &[]);
   }
   ```
   （注意 `respond_static` 的缓冲/头部已处理 Content-Length；不要再走 `respond()` 的 JSON 路径。）
2. 新增 `fn metrics_text(app: &Arc<App>) -> String`：
   - 每行 `name{labels} value`，每个指标前必须有 `# HELP` 与 `# TYPE`（gauge/counter 二选一）；
   - 数字用整数或 `{:.3}` 浮点，**禁止输出 NaN/Inf**；空 state 也必须是合法文本（计数全 0）；
   - body 以 `\n` 结尾；标签值按 Prometheus 规则转义 `\` `"` `\n`（写一个 `fn esc_label(&str) -> String` 并单测）。

## 三、指标集合（名字与类型固定，全部实现）

| 指标 | 类型 | 来源 |
|---|---|---|
| `roamway_build_info{version="<CARGO_PKG_VERSION>"}` = 1 | gauge | `env!("CARGO_PKG_VERSION")` |
| `roamway_uptime_seconds` | gauge | `now() - app.started` |
| `roamway_runs{status="queued"\|"running"\|"done"\|"failed"\|"timeout"\|"killed"\|"interrupted"}` | gauge | `state.runs` 按 `RunStatus` 全枚举 match 计数（7 行都要输出，缺的写 0） |
| `roamway_agents{state="<state>"}` | gauge | `state.agents` 按 Agent 的状态字段分组（字段名以 `src/types.rs` 为准，`status_route` 里序列化成 `state`），按出现过的状态出标签，无 agent 时输出一行 `roamway_agents 0` 之类合法形式 |
| `roamway_jobs` / `roamway_jobs_enabled` | gauge | `state.jobs` 总数 / `enabled == true` 数 |
| `roamway_schedules` / `roamway_schedules_paused` | gauge | `app.schedules` 总数 / `enabled && !paused` 互补计数 |
| `roamway_schedule_failures_total` | counter | Σ `consecutive_failures`（HELP 里注明「daemon 内存态，重启归零」） |
| `roamway_terminals` | gauge | `app.terms.lock().len()` 语义（以 `terminals.rs` 实际存储为准） |
| `roamway_http_concurrent_requests` | gauge | `app.conns.load(Ordering::Relaxed)` |
| `roamway_http_requests_total` | counter | 新增 `App.req_total: AtomicU64`，`handle()` 每次请求 +1 |
| `roamway_http_requests_failed_total` | counter | 新增 `App.req_failed: AtomicU64`，响应码 ≥ 400 时 +1（在构造响应的统一出口加，别散落各处） |

新增的两个 `AtomicU64` 必须：`src/types.rs` 里默认 0、4 处构造点显式 `AtomicU64::new(0)`、CLI 侧不参与持久化。

## 四、测试与文档

1. 单测（`src/server.rs` 的 `mod tests`）：
   - `metrics_text` 对空 state 输出合法文本：body 以 `\n` 结尾、含全部 11 个指标名、无 `NaN`；
   - 塞入 1 个 done run + 1 个 running run + 1 个 enabled job + 1 个 paused 调度后，对应计数正确；
   - `esc_label` 对 `a"b\c` 与换行的转义正确；
   - 若测试里能直接驱动 `handle`/`route`，再加一条「无 token → 401，带 token → 200 且 CT 含 `version=0.0.4`」。
2. 文档：README 的 Roadmap 勾选 `/metrics`，并新增 `## 监控` 小节，给最小 scrape 配置示例
   （`metrics_path: /metrics`、`bearer_token` 或 `params: {token: [...]}`），说明「先 `roamway token add --name prometheus --scope read --ttl 0`」。
3. 不在本阶段做：直方图/时延分布、`/metrics` 免鉴权开关、外部 exporter 依赖。

## 五、验收（NAS）

1. `cargo test` 全绿，新增 ≥4 条单测。
2. 生产 8796（部署后）：
   - `curl -s -H "Authorization: Bearer $TOK" localhost:8796/metrics | head -30` → 文本，首行是 `# HELP`
   - `curl -s -o /dev/null -w '%{http_code}' localhost:8796/metrics` → 401（未带 token）
   - `curl -s localhost:8796/metrics?token=$TOK | grep -c '^roamway_'` → ≥ 11
   - 触发一次 `roamway run --agent shell "echo p18"` 后再抓，`roamway_runs{status="done"}` 与
     `roamway_http_requests_total` 单调不减。
3. 不改任何既有路由的响应形状；P1-P3 已验收行为不变。
