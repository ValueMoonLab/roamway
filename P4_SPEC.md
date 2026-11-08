# P4 规范：工作区、克隆与并行隔离（PARITY P4）

Paseo 这层只有一条顶层命令 `clone`，但「并行 agent 互不踩」是它真正的价值点。roamway 要补两件事：`clone` 和 **per-agent worktree 隔离**。

## 1. `clone`（对齐 Paseo 语义）

```
roamway clone <owner/repo | git url> [--dir <parent>] [--protocol https|ssh] [--json] [--host]
```

- `owner/repo` 简写按 `--protocol`（默认 https）拼成 `https://github.com/owner/repo.git` / `git@github.com:owner/repo.git`
- 目标目录：`<dir>/<repo basename>`，`--dir` 缺省用 `<data>/workspaces`
- 执行 `git clone --depth 0`（即完整克隆）；非 git 环境或网络失败 → 明确报错，不留半成品目录
- 成功后登记到 `<data>/workspaces.json`：`{id: ws_xxxx, name, path, remote, host: "local", created_at}`
- 输出：`cloned <name> → <path>`；`--json` 输出登记记录

## 2. 工作区注册表（`src/workspaces.rs` 新增）

```rust
pub struct Workspace { pub id: String, pub name: String, pub path: String, pub remote: String, pub host: String, pub created_at: i64 }
impl Registry { load(dir) / save() / add(path, remote) -> Workspace / get(name_or_path) / remove(name) }
```

- `run --workspace` 接受：绝对路径、相对路径、已登记的工作区名
- `roamway workspace ls [--json]` / `roamway workspace rm <name>`（新增，Paseo 没有但运维需要；只在帮助的扩展区列出）

## 3. per-agent worktree 隔离

`agent run --worktree`（P3 完成后接入）：

1. 解析工作区为 git 仓库根（`git rev-parse --show-toplevel`），不是 git 仓库 → 报错 `workspace is not a git repo; drop --worktree`
2. 创建 `git worktree add <data>/worktrees/<agent_id> -b rw/<agent_id>`（基线为当前 HEAD）
3. agent 的 cwd = 该 worktree；agent 记录里加 `worktree: true, branch: rw/<agent_id>, base_path: <原仓库>`
4. `agent archive` 或 `agent rm` 时：`git worktree remove --force` + `git branch -D rw/<agent_id>`（`--keep-worktree` 可跳过）
5. 同一仓库多个 worktree 并行时互不影响；分支名唯一

## 4. 单元测试

- `clone_url_build_https_and_ssh`
- `clone_rejects_partial_dir_on_failure`（模拟无 git / 坏 URL，断言不残留目录）
- `workspace_registry_roundtrip`
- `workspace_lookup_by_name_and_path`
- `worktree_path_is_under_data_dir`
- `worktree_branch_name_unique_per_agent`

## 5. 验收（NAS）

1. `roamway clone valuemoonlab/roamway --dir /home/GZX/ws --json` → 目录存在、`workspaces.json` 有记录
2. `roamway clone bad/does-not-exist` → 报错且不残留目录
3. 在 roamway 仓库上起 3 个 `agent run --worktree` 并发写不同文件 → 三个 worktree 各自 1 个新文件，主仓库 `git status` 干净
4. `agent archive` 后 worktree 目录与 `rw/<id>` 分支消失
5. `agent run --worktree` 指向非 git 目录 → 明确报错
6. `cargo build --release` 0 warning，`cargo test` 全过

## 6. 约束

- worktree 全部落在 `<data>/worktrees/` 下，不在用户仓库里制造垃圾
- 删除操作一律先 dry-run 打印，再执行；失败不吞错误
- 支持 `--host`：远端 clone/worktree 由远端 daemon 执行
