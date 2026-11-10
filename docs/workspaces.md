# Worktree 工作区隔离

`run --new-workspace worktree` 在独立 git worktree 中执行 agent，避免污染当前分支。本文说明 P16 的命名、基点与清理语义；实现见 `src/workspaces.rs`。

## 命名

- 默认目录：`<data>/worktrees/<agent_id>`；`--worktree-slug <slug>` 时使用 `<data>/worktrees/<slug>`（`src/workspaces.rs:fn worktree_path_named`）。
- `--worktree-slug` 合法形式 `[a-z0-9][a-z0-9-]{0,39}`（1..=40 字符，首字符不能是 `-`）；与已有 worktree 目录或分支同名时拒绝创建。
- 默认新分支：`rw/<agent_id>`；`--new-branch <name>` 替换为 `<name>`，合法形式 `[A-Za-z0-9._/-]{1,100}` 且不以 `-` / `/` 开头或结尾。

## base 语义

- 新分支起点缺省 `HEAD`；`--base <ref>` 指定起点，仅 `branch-off` 模式生效。
- `<ref>` 必须能被 `git rev-parse --verify <ref>^{commit}` 解析，否则 exit 2 `run --base: unknown ref: <ref>`。
- 创建命令固定为 `git -C <root> worktree add <path> -b <branch> [<base>]`（`src/workspaces.rs:fn worktree_add_from`）；`--branch <ref>` 走 `git worktree add <path> <ref>` 检出已有分支。

## 清理

- 每次 worktree run 会把 `path` / `branch` / `base` 记录到 `run.worktree`（`src/workspaces.rs:struct WorktreeInfo`）。
- `archive <id>` 时优先按记录删除（`src/agents.rs:fn archive_agent`），记录缺失时回落到旧的 `<data>/worktrees/<agent_id>` 推导，旧 state.json 仍可删干净。
- `branch-off` 删除 worktree 并删除新建分支；`checkout-branch`（含 `--branch`）删除 worktree 但保留已有分支。

## inspect

`roamway inspect <id>` 对 worktree agent 额外打印一行：

```text
worktree:   <path> (<branch>, base=<base|HEAD>)
```