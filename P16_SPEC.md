# P16_SPEC — worktree 选项（slug / new-branch / base / branch）+ PARITY #15 收口

前置事实（实测于 P15 之后的 HEAD，锚点为文件:函数）：

- `src/main.rs:246` 的 `run_plan` 不支持清单里还剩 `worktree-slug`、`new-branch`、`base`、`branch` 四项
  （`--new-workspace worktree` 与 `--worktree-mode {branch-off,checkout-branch,checkout-pr}` + `--pr-number` 已能用，别动）。
- `src/agents.rs:268-290 fn create_agent_and_run`：worktree 三路分支——
  `checkout-pr` → `workspaces::worktree_add_existing(&root, &path, &branch)`；
  `branch-off`（默认）→ `worktree_add(&root, &path, &workspaces::worktree_branch(&id))`；
  路径一律 `workspaces::worktree_path(&data_dir, &id)` = `<data>/worktrees/<agent_id>`。
- `src/workspaces.rs:250 worktree_path`、`:254 worktree_branch`（`rw/<agent_id>`）、
  `:274 worktree_add` → 固定执行 `git -C <root> worktree add <path> -b <branch>`（**基点是 HEAD，无法指定 base**）、
  `:339 worktree_add_existing` → `git worktree add <path> <branch>`。
- 清理路径 `workspaces.rs:302 worktree_remove` / `:354 worktree_remove_keep_branch` 按 `worktree_path(agent_id)` 反查，
  **改成自定义 slug 后必须仍能删除**，否则会漏清。

本阶段目标：4 个 worktree 参数落地，命名/基点在**创建时**生效，且清理路径与新命名保持一致；
同时把 PARITY #15（`daemon run` 参数集）核验收口。

---

## 一、语义

1. `--worktree-slug <slug>`：worktree 目录名用 `<slug>` 取代 `<agent_id>`；
   合法 `[a-z0-9][a-z0-9-]{0,39}`，否则 exit 2 `run --worktree-slug: expected [a-z0-9-], max 40`。
   与**已有** worktree 目录/分支同名 → exit 2 `run --worktree-slug: already exists: <slug>`（不覆盖）。
   未给 `--new-workspace worktree` 时给 slug → exit 2 `run --worktree-slug requires --new-workspace worktree`。
2. `--new-branch <name>`：新建分支名取代 `rw/<agent_id>`；校验 `git check-ref-format --branch` 语义
   （`[A-Za-z0-9._/-]{1,100}`、不以 `-`/`/` 开头或结尾），非法 → exit 2 `run --new-branch: invalid branch name: <n>`。
3. `--base <ref>`：新分支的起点，默认 `HEAD`。必须能 `git rev-parse --verify <ref>^{commit}` 解析，否则 exit 2
   `run --base: unknown ref: <ref>`。只对 `branch-off` 生效；配 `checkout-pr`/`checkout-branch` → exit 2 `run --base: only valid with branch-off`。
4. `--branch <name>`：在工作树里检出**已有**分支（等价 `worktree_add_existing`）。
   与 `--new-branch` 或 `--base` 同时给 → exit 2 `run --branch conflicts with --new-branch/--base`。
   分支不存在 → exit 2 `run --branch: no such branch: <name>`。
5. 上述校验全部在**创建 worktree 之前**完成（CLI 侧 + server 侧各一次，文案逐字一致）。

## 二、数据模型与实现

1. `src/workspaces.rs` 新增：
   ```rust
   pub struct WorktreeSpec { pub slug: Option<String>, pub branch: Option<String>, pub base: Option<String>, pub existing: bool }
   pub fn worktree_path_named(data_dir: &Path, agent_id: &str, slug: Option<&str>) -> PathBuf
   pub fn worktree_add_from(repo_root: &str, path: &Path, branch: &str, base: Option<&str>) -> Result<(), String>
   ```
   `worktree_add_from` 的 argv 固定 `git -C <root> worktree add <path> -b <branch> [<base>]`；
   `worktree_add` 保留为 `worktree_add_from(.., None)` 的薄包装（**既有调用点与测试不动**）。
2. `agents::create_agent_and_run` 读取 `Run`/plan 上的 spec，走 `worktree_path_named` + `worktree_add_from` / `worktree_add_existing`；
   `Run` 新增 `#[serde(default, skip_serializing_if = "Option::is_none")] worktree: Option<WorktreeInfo>`，
   `WorktreeInfo { path, branch, base: Option<String> }`，供清理与 inspect 反查。
3. 清理：`worktree_remove` 的 path/branch 来源改为 agent 上的 `worktree` 记录，缺失时回落到旧的 `agent_id` 推导
   （旧 state.json 里的 agent 必须仍能删干净）。
4. CLI/HTTP：`run_plan` 移出这 4 项的硬拒绝；`POST /v1/runs` 接受 `"worktree": {"slug"|"branch"|"new_branch"|"base"}`。
5. `roamway agent inspect <id>` worktree agent 增一行 `worktree: <path> (<branch>, base=<base|HEAD>)`。

## 三、单测

1. `worktree_spec_parses_slug_branch_base`（4 组，含互斥与非法值）
2. `worktree_add_from_argv_includes_base`（构造 argv 的纯函数 `worktree_add_argv(path, branch, base)`）
3. `worktree_path_named_uses_slug_and_falls_back_to_agent_id`
4. `worktree_remove_uses_recorded_path`（构造 agent + 记录，断言 dry-run 行里的路径/分支）
5. `run_plan_rejects_slug_without_worktree_workspace`

## 四、文档

- `docs/cli.md`：4 个参数进 `run` 表；`docs/workspaces.md`：命名规则、`base` 语义、清理行为。
- `PARITY.md`：#3 `paseo run` 参数集在此收口 → ✅（证据：`provider_args`、`worktree_add_from` + 测试名）；
  #15 `daemon run` 核验：与 `docs/config.md`（P14 产出）逐项对照，一致则 ✅ 并写测试名/文档锚点，不一致则就地补文档，
  **不得为了对齐而改已验收行为**。

## 五、验收（keeper 实测，NAS）

- NAS `cargo build --release && cargo test` 全绿、`WARN_COUNT=0`。
- 建临时仓库 `/tmp/p16repo`（2 个 commit，`main` + `base1` 分支）：
  - `roamway run --agent shell --new-workspace worktree --worktree-slug p16-a --new-branch feat/p16 --base base1 "echo ok"`
    → `git -C /tmp/p16repo worktree list` 含 `p16-a`；`git -C /tmp/p16repo rev-parse feat/p16` == `base1` 的 commit。
  - `--branch main`（已存在）→ 检出该分支而不是新建。
  - `--worktree-slug p16-a` 再跑一次 → exit 2 `already exists`；`--branch main --base base1` → exit 2 冲突；
    `--base nope` → exit 2 `unknown ref`；`--worktree-slug Bad_Slug` → exit 2。
  - 删除该 agent 后 `git worktree list` 不再含 `p16-a`，分支 `feat/p16` 一并删除（旧行为保持）。
- 生产 8796 冒烟：默认 `run --agent shell "echo p16"` 与 `--new-workspace worktree`（无新参数）行为与 P15 一致。
- `PARITY.md` 只剩 #15 之外无 ⚠️ 行时，在 `README.md` 的路线图里标注"参数面对齐完成（P15+P16）"。
