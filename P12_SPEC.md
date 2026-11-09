# P12_SPEC — 运行面语义对齐（ls 默认 agent 维度 / run 支持 worktree·cwd·env·label）

前置事实（全部实测于 85982ef，不是引用旧文档）：

- `src/main.rs:990 fn cmd_ls` 打 `GET /v1/runs?limit=<n>&trigger=<t>`，列 `ID/STATUS/AGENT/MODEL/CREATED/TITLE`；
  `-q` 输出每行一个 run id。run 维度，**不是** Paseo 的 agent 维度。
- `src/main.rs:2911 fn cmd_agent_ls` 是 agent 维度，但只认 `--all`（拼 `/v1/agents?all=1`）和 `--json`；
  无 `-q/--no-headers/--limit/--label/--global/--format`。
- `src/server.rs:1608 fn list_agents_route` 只读 `all` 一个 query 参数，过滤逻辑在
  `agents::filter_visible(&st.state.agents, all)`（`src/agents.rs`），无 label/limit/kind 过滤。
- `src/main.rs:909 fn cmd_run` 打 `POST /v1/runs`，body 固定五个字段
  `{agent, model, workspace, title, prompt, timeout_s}`（`src/server.rs:744 struct NewRun`，`deny_unknown_fields`），
  无 worktree / label / env / cwd；响应只有 `{"run": ...}`，**不回 agent id**。
- worktree 能力已存在，但只在 agent 路径：
  `agents.rs:199 create_agent_and_run(..., use_worktree: bool)` → `workspaces::worktree_path/worktree_branch/worktree_add`
  （`src/workspaces.rs:246-298`）；`agents.rs:359 archive_agent(app, id, keep_worktree)` 归档时
  `worktree_remove` 连分支一起删（`workspaces.rs:302`，`git worktree remove --force` + `git branch -D`）。
  `NewAgent`（`server.rs:805`）已有 `worktree: Option<bool>`。
- `Run`（`src/types.rs:119`）与 `Agent`（`src/agents.rs:16`）都**没有** label 字段，也没有 env 字段。
- Paseo 0.15.0 真实选项（`docs/paseo-cli-surface.md`）：`ls` 有 `-a/--all`、`-g/--global`、`--background`、
  `--label k=v`、`--json/-o/-q`；`run` 有 `--new-workspace local|worktree`、
  `--worktree-mode branch-off|checkout-branch|checkout-pr`、`--worktree-slug`、`--new-branch`、`--base`、`--branch`、
  `--pr-number`、`--provider`、`--cwd`、`--env`、`--label`、`--wait-timeout`、`--thinking/--mode/--image/--output-schema`。

本阶段目标：把 roamway 的 `ls` / `run` 从「run 维度薄封装」提升到 Paseo 的「agent 维度 + worktree 隔离」语义，
且**不删除任何旧能力**（旧 run 表用 `--runs` 复现）。不得改 P1–P3 已验收的 HTTP 行为：`POST /v1/runs` 的
请求/响应形状保持现状，`run` 的输出契约（首行 run id）保持现状。

---

## 一、`ls` 默认 agent 维度（src/main.rs + src/server.rs）

1. `roamway ls` 默认 → agent 维度，列 `ID/STATE/KIND/MODEL/TITLE/UPDATED`（复用 `cmd_agent_ls` 的列宽与 `iso()`
   时间格式）。`roamway ls --runs` → 现有 run 表（`cmd_ls` 原样保留，只是入口改名/分支）。
2. `ls` 新增参数（全部同时作用于 agent 维度）：
   - `-a/--all`：含 archived（`?all=1`）
   - `-g/--global`：跨 host 聚合，复用 `cmd_ls_all`（`main.rs:2567`）的现有聚合路径，不得复制一套 HTTP 逻辑
   - `--label k=v`（可重复）：转成 `?label=k%3Dv`，多值重复查询参数
   - `--limit N`：`?limit=N`，默认 50，clamp 1..=500
   - `-q/--quiet`：只打 id，每行一个
   - `--no-headers`：省略表头
   - `--format json|yaml` / `--json`：输出原始响应（`{"agents": [...]}`），三者一致
   - `--background`：**不做**（roamway 无后台列表模型），`usage_error` 提示 `ls --background: not supported` + exit 2，
     PARITY.md 记入「不做」并给理由
3. 服务端 `GET /v1/agents` 新增 query（保持 `{"agents": [...]}` 响应形状不变，向后兼容）：
   - `label=k=v`（可重复，全部命中即匹配，AND 语义）
   - `limit`（默认不限=现有行为，给值才截断，clamp 1..=500）
   - `kind=codex|claude|shell|api:*`（精确匹配）
   - 过滤必须发生在 `list_agents_route`，按 `created_at` 降序；CLI 只传参，不在客户端二次过滤（`-g` 除外）。
4. `labels` 数据模型：`Agent.labels: BTreeMap<String,String>`（`#[serde(default, skip_serializing_if =
   "BTreeMap::is_empty")]`），`Run.labels` 同构。`create_agent_route` 接受 `labels` 并落到 agent 与首个 run。
   `Run` 新增字段意味着**所有** `Run { .. }` 构造点都要补（`server.rs` 的 `create_run`/`create_agent_and_run` 调用链、
   `agents::create_agent_and_run`、`agents::queue_agent_run`、`agents::reload_agent`、`schedules.rs` 的触发路径），
   用 `..Default::default()` 不可行（`Run` 未 derive Default）——逐个补齐并用 `cargo test` 兜底。

## 二、`run` 改为 agent-backed（src/main.rs:cmd_run）

1. `run` 内部改打 `POST /v1/agents`（body 含 `kind`、`model`、`workspace`、`title`、`prompt`、`timeout_s`、
   `worktree`、`labels`、`env`、`cwd`）。**输出契约不变**：
   - 默认等待结束，首行 `<run_id>\t<status>\t<N>s[\t<error>]`，随后 `---` 与日志尾（沿用现有格式）
   - `--no-wait` 只打 `<run_id>`
   - 因此服务端响应 `{agent, run}` 里必须取 `run.id`，不得打 agent id
2. 新增参数：
   - `--new-workspace local|worktree`：`worktree` → `worktree: true`（走现有 `worktree_add`）；
     `local` → 现状（直接在工作区跑）；其它值 → exit 2
   - `--worktree-mode branch-off|checkout-branch|checkout-pr`：
     - `branch-off`（默认，与 `--new-workspace worktree` 组合）：复用现有 `worktree_add`（新分支 `rw/<agent_id>`）
     - `checkout-branch`：在**已存在**分支上建 worktree（`git worktree add <path> <branch>`，不带 `-b`）；
       归档时只删目录、**保留分支**（`archive_agent(keep_worktree=true)` 的语义扩展为 `keep_branch`）
     - `checkout-pr`：`gh pr checkout <n>`（需要 `--pr-number`）；`gh` 不在 PATH → stderr
       `checkout-pr requires gh in PATH` + exit 2。不得自己用 `git fetch refs/pull/*` 伪造
   - `--cwd DIR`：run 的工作目录；服务端用现有 `canonicalize_dir` 校验，不存在 → 400，CLI exit 1；
     与 `--workspace` 同给时 `--cwd` 覆盖执行目录、`--workspace` 仍是 agent 的 workspace 归属
   - `--env K=V`（可重复）：落到 `Run.env: Vec<(String,String)>`（serde 默认空），`runner::run_cli` spawn 时注入，
     `K` 必须是 `[A-Za-z_][A-Za-z0-9_]*`，非法 → exit 2；**日志里不得回显值**，只回显 key 名
   - `--label k=v`（可重复）：同 `ls` 的解析规则，非法 → exit 2
   - `--wait-timeout N`：本地轮询上限（秒）。超时 → stderr `wait timeout after Ns; run <id> still <status>` + exit 1，
     **不得** stop 远端 run
3. 明确不支持：`--thinking`、`--mode`、`--image`、`--output-schema`、`--provider`（roamway 用 `--agent`）、
   `--worktree-slug`、`--new-branch`、`--base`、`--branch` → 统一 `stderr: run --<flag>: not supported by roamway`
   + exit 2，并在 PARITY.md 记「未对齐」及理由。**不许静默忽略，也不许凭空实现一套假的语义。**
4. `POST /v1/runs` 保持原样；`run` 不再调它（`run --runs` 不需要）。`cmd_run_remote`（`main.rs:2449`）同步改为
   走远端 `POST /v1/agents`，保持首行 run id。

## 三、单测（必须新增，纯函数优先）

1. `fn parse_labels(vals: &[String]) -> Result<BTreeMap<String,String>, String>`：`["a=1","b=2"]` 命中；
   `["a"]`、`["=x"]`、`["a=b=c"]`（值含 `=` 视为合法，key 不能含 `=`）各有断言。
2. `fn run_plan(flags) -> Result<RunPlan, String>`：`--new-workspace worktree` → worktree=true；
   `--worktree-mode checkout-pr` 缺 `--pr-number` → Err；`--thinking` → Err 且错误串含 `not supported`。
3. `fn ls_plan(flags) -> LsPlan`：`--runs` 切 run 维度；`-g` 优先于 host；`--limit abc` → 默认 50。
4. `fn agent_list_url(plan) -> String`：label 多值 URL 编码正确（`k=1` → `label=k%3D1`），limit/kind 拼接顺序断言。
5. 服务端过滤：把 `list_agents_route` 的过滤抽成 `fn filter_agents<'a>(agents, all, labels, kind, limit) -> Vec<&'a Agent>`，
   用 3 个构造 Agent 断言 label AND 语义与 limit 截断。

## 四、文档（同一提交）

- `PARITY.md`：第 2 行（`ls`）、第 3 行（`run`）改为 ✅ 或 ⚠️，证据写 `src/main.rs:fn run_plan` / `测试：<name>`；
  `--background`、`--thinking` 等不支持项进「不做」列并写理由。
- `README.md` 命令一览：`ls` 标注「默认 agent 维度，`--runs` 看运行记录」；`run` 给出 worktree 三行示例。
- `docs/quickstart.md`：`serve → run → ls` 流程按新语义改（`ls` 现在列 agent）。

## 五、验收（keeper 逐条实测，NAS）

- `source ~/.cargo/env && cargo build --release && cargo test` 全绿，`cargo build --release 2>&1 | grep -c warning` = 0。
- 在部署机源码目录 `$RW_SRC` 上：
  - `roamway ls` 表头首列是 `ID`、第二列 `STATE`；`roamway ls --runs` 出现 `STATUS` 列
  - `roamway run --new-workspace worktree --label stage=p12 --env FOO=1 "echo hi"` → 退出码 0，
    `ls --runs` 里 run 状态 done，`git -C $RW_SRC worktree list` 出现 `…/worktrees/ag_…`
  - `roamway ls --label stage=p12` 命中该 agent；`roamway ls --label stage=nope` 为空表（非报错）
  - `roamway run --thinking high "x"` → rc=2；`roamway ls --background` → rc=2
  - `roamway run --no-wait "echo hi"` 只输出一行且是 `run_` 前缀 id
