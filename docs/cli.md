# CLI 参考

本文维护 `roamway run` 的参数表与校验规则；权威文本以 `roamway run --help` 输出为准。其余命令面见 `docs/paseo-cli-surface.md`。

## run

`roamway run --agent <kind> [options] "<prompt>"`

| 参数 | 取值 | 说明 |
|---|---|---|
| `--agent` | `codex` / `hermes` / `claude` / `shell` / `api:*` | agent 类型，缺省 `codex` |
| `--new-workspace` | `local` / `worktree` | 新建本地目录或 worktree；`worktree` 时启用分支隔离 |
| `--worktree-mode` | `branch-off` / `checkout-branch` / `checkout-pr` | worktree 创建模式，缺省 `branch-off` |
| `--worktree-slug` | `[a-z0-9][a-z0-9-]{0,39}` | worktree 目录名，替换 `<agent_id>`；最长 40；需 `--new-workspace worktree`；同名已存在 exit 2 |
| `--new-branch` | `[A-Za-z0-9._/-]{1,100}` | 新分支名，替换 `rw/<agent_id>`；最长 100，不以 `-` / `/` 开头或结尾；非法 exit 2 |
| `--base` | `<ref>` | 新分支起点，缺省 `HEAD`；仅 `branch-off`；需能被 `git rev-parse --verify <ref>^{commit}` 解析，否则 exit 2 |
| `--branch` | `<ref>` | 检出已有分支；与 `--new-branch` / `--base` 互斥；分支不存在 exit 2 |
| `--pr-number` | `<n>` | `checkout-pr` 必填；依赖 `gh pr checkout` |
| `--thinking` | `none` / `low` / `medium` / `high` | per-run 透传，映射见 `docs/providers.md` |
| `--mode` | `read-only` / `workspace-write` / `danger-full-access` | per-run 透传，映射见 `docs/providers.md` |
| `--provider` | 见 `docs/providers.md` | per-run 透传，provider 无能力时 exit 2 |
| `--image` | `<path>`（可重复） | 附加图片；路径必须可读 |
| `--output-schema` | `<path>` | 需为含 `"type"` 的 JSON 对象文件 |
| `--cwd` | `<dir>` | agent 工作目录，缺省当前目录 |
| `--env` | `K=V`（可重复） | 注入进程环境变量 |
| `--label` | `k=v`（可重复） | agent 标签 |
| `--workspace` | `<id 或路径>` | 在既有工作区中运行 |

### 校验规则

- `--worktree-slug` 非法 → exit 2 `run --worktree-slug: expected [a-z0-9-], max 40`。
- `--worktree-slug` 与已有 worktree 目录/分支同名 → exit 2 `run --worktree-slug: already exists: <slug>`。
- 未给 `--new-workspace worktree` 时给 `--worktree-slug` → exit 2 `run --worktree-slug requires --new-workspace worktree`。
- `--new-branch` 非法 → exit 2 `run --new-branch: invalid branch name: <name>`。
- `--base` 无法解析 → exit 2 `run --base: unknown ref: <ref>`；配 `checkout-pr` / `checkout-branch` → exit 2 `run --base: only valid with branch-off`。
- `--branch` 与 `--new-branch` / `--base` 同时给 → exit 2 `run --branch conflicts with --new-branch/--base`；分支不存在 → exit 2 `run --branch: no such branch: <name>`。

HTTP 侧 `POST /v1/agents` / `POST /v1/runs` 接受 `"worktree": true|false` 或 `"worktree": {"slug"|"branch"|"new_branch"|"base"}`，校验失败返回 400 且错误文本逐字一致。