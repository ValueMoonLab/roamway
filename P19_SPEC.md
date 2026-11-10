# P19_SPEC — 任务依赖（A 成功后再跑 B）

前置事实（实测于 P16 之前的 HEAD `eedfe2f`）：

- `src/types.rs:177 pub struct Job`：字段 `id/title/agent/model/workspace/prompt/every_secs/daily/tz_offset_hours/
  timeout_s/max_retries/enabled/allow_overlap/next_at/last_run/last_error/…`，全部可 serde 序列化进 `state.json`。
- 调度触发点：`src/jobs.rs:44 fn start_scheduler` 每轮调 `jobs.rs:54 fn collect_due(app: &Arc<App>) -> Vec<String>`；
  现有唯一闸门是「同 id 有未终结 run 且 `!allow_overlap` → `next_at = t + 5` 跳过一次」（`jobs.rs:71`）。
  命中后 `jobs.rs:80 next_at = next_after(j, t, tz)`（推进到下一周期），`:82 due.push`。
- 实际起 run：`src/jobs.rs:87 fn spawn_from_job`；`last_run` 在 `:129` 写入。
- 建任务：CLI `src/main.rs:1686 fn cmd_jobs_add`（`--every`/`--daily`/`--allow-overlap`… 拼 body 后 `POST /v1/jobs`），
  HTTP `src/server.rs` 的 `create_job`（路由 `:1068 ("POST", ["v1","jobs"]) => create_job(app, req)`）。
- 列表渲染：CLI `src/main.rs:1654 fn cmd_jobs`（表头 `ID / ON / AGENT / EVERY / NEXT / LAST / TITLE`），
  HTTP `src/server.rs:1064 ("GET", ["v1","jobs"])` 直接吐 `st.state.jobs` 全量对象（新字段自动出现在 JSON 里）。

本阶段目标：任务可声明前置任务，前置**最近一次运行成功**后才允许触发；不满足则跳过本轮且记原因；手工 `jobs-run` 显式绕过。

---

## 一、数据模型

1. `src/types.rs` `Job` 新增：
   ```rust
   #[serde(default, skip_serializing_if = "Vec::is_empty")]
   pub depends_on: Vec<String>,
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub last_skip: Option<String>,
   ```
   `last_skip` = 最近一次「因依赖未满足而跳过」的原因字符串 `waiting for <dep_id> (<reason>)`，成功触发时清 `None`。
   旧 `state.json`（无这两个字段）必须能加载 —— 既有 serde default 惯例；`Job` 的所有构造点（`create_job`、测试 `sample_job`）都要补字段。
2. 依赖语义：job B 的 `depends_on = [A, …]`，判定 A「已满足」＝ A 的最新一条 run（按 `created_at` 取 max，`state.runs` 里 `job_id == A`）
   存在且 `status == RunStatus::Done`。不存在或最后一条是 Failed/Timeout/Killed/Interrupted/Queued/Running → 不满足。

## 二、校验（服务端权威，CLI 侧同样文案）

1. `--after <id>` 可重复；空串/不存在 → 400 `jobs-add --after: no such job: <id>`。
2. 自引用 → 400 `jobs-add --after: self reference`。
3. 成环（含间接）→ 400 `jobs-add --after: dependency cycle: <a> -> <b> -> <a>`（DFS 从待建节点出发，
   遇到回边即报，路径按访问顺序拼接）。
4. 校验必须在写入 store 之前完成；失败不落库。

## 三、行为

1. `collect_due`：在现有 `!j.enabled || j.next_at > t` 之后、`busy` 判断之前插入依赖判定：
   不满足 → `next_at = next_after(j, t, j.tz_offset_hours)`（照常推进，**不 5s 重试**，避免空转），
   `last_skip = Some("waiting for <dep_id> (<status>)")`，不进 `due`，不 `spawn_from_job`。
   满足 → `last_skip = None` 并照原逻辑触发。
2. `POST /v1/jobs/{id}/run`（手工触发，路由 `:1069` 附近）：**绕过**依赖；若依赖未满足，响应 JSON 追加
   `"deps_bypassed": true`，CLI `src/main.rs:1719 fn cmd_jobs_run` 在非 `--json` 模式下打印
   `note: dependencies not satisfied, running anyway`。依赖已满足时不出现该字段。
3. `roamway jobs` 表加一列 `AFTER`（`depends_on` 用 `,` 连接，空为 `-`），`LAST` 列保持不变；
   行尾若 `last_skip` 非空则在其后追加 `  ! <last_skip>`（不新增列，避免破坏对齐）。
4. `GET /v1/jobs` 无需改动（全量对象自动带新字段）。

## 四、测试（`src/jobs.rs` 的 `mod tests` + `src/server.rs` 校验函数单测）

1. `collect_due`：dep 无 run → 不 due 且 `last_skip` 有值、`next_at` 已推进；
2. dep 最后一条 Done → due；dep 最后一条 Failed → 不 due；
3. 依赖两条（都 Done 才 due）；
4. 环检测：A→B→A、自引用分别报对应错误；
5. 旧 state.json 反序列化（无 `depends_on`）后 `depends_on.is_empty()`，其余字段不丢。

## 五、验收（NAS，生产 8796）

```sh
TOK=…  # 现有 token
A=$(roamway jobs-add --agent shell --every 300 --title depA "exit 1" | grep -o '"id": "[^"]*"' | head -1 | cut -d'"' -f4)
B=$(roamway jobs-add --agent shell --every 300 --after "$A" --title depB "echo depB-ran" | …)
roamway jobs-run "$A"        # 先让它失败一次
roamway jobs                 # 期望 B 行 AFTER=<A>，且 B 的 last_skip 出现 waiting for <A> (failed)
roamway jobs-run "$A"        # 这条 prompt 仍是 exit 1，需换成功任务验证通过路径
```
更稳的做法：A 的 prompt 用 `test -f /tmp/p19.ok` 风格（先不存在→失败，`touch /tmp/p19.ok` 后→成功），
验证 B 在 A 成功后当轮被触发（B 的 `last_run` 变非空、`last_skip` 清空）。
最后 `roamway jobs-rm` 清掉 A/B，避免污染生产调度。

约束：不改 P1-P3 已验收行为；不动 schedule（`src/schedules.rs`）语义；不引入外部依赖。
