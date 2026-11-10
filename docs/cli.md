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

## backup

`roamway backup [--out DIR] [--keep N]`

- 仅本地执行：显式带 `--host` → exit 2 `backup/restore are local-only`；`--data`/`--home` 生效。
- `--out` 缺省 `<data>/backups`；在其下新建 `roamway-backup-<YYYYmmdd-HHMMSS>/`，同秒重复追加 `-2`、`-3`。
- 只复制存在的 `state.json`、`config.json`、`schedules.json`，外加 `manifest.json`；绝不打包 token/pairing/passwords 等凭据。
- `manifest.json` 格式：`{"format":"roamway-backup/1","created_at":<unix>,"version":"<crate 版本>","files":[{"name":"state.json","bytes":123,"sha256":"..."}]}`。
- `--keep N`：备份成功后按目录名保留最新 N 个 `roamway-backup-*`，`0`=不清理；`N` 非法 → exit 2。
- table 输出 `BACKUP_DIR FILES BYTES`；`-o json` 输出 `{"ok":true,"dir":"...","files":3,"bytes":N,"pruned":N}`。

## restore

`roamway restore --from DIR [--force] [--yes]`

- 仅本地执行：显式带 `--host` → exit 2；`--data`/`--home` 生效。
- `DIR` 必须含 `manifest.json` 且 `format == "roamway-backup/1"`；逐文件校验存在、`bytes`、`sha256`，任一不符 → exit 1 且一个字节都不写。
- daemon 存活检测（任一证据命中即拒）：`<data>/roamway.pid` 对应进程存活 → `pid ...`；`<data>/heartbeat` 60 秒内有更新 → `heartbeat ...s ago`；`<data>/bind` 指向端口可连接 → `port ... accepts connections`。
- 命中检测且无 `--force` → exit 3，且一个字节都不写；`--force` 只跳过停服检查，不是热替换，仍会做 `*.pre-restore-*` 轮换，可能被运行中 daemon 下一次 save 覆盖。
- 写前把现有 `state.json` 等轮换为 `*.pre-restore-<YYYYmmdd-HHMMSS>`；写入顺序 config → schedules → state。
- 不删除备份源目录；table 输出 `RESTORED FROM`；`-o json` 输出 `{"ok":true,"files":[...],"from":"..."}`。

## 退出码补充

`backup`/`restore`：`0` 成功；`1` 目录不可写、复制失败、校验失败或远端失败；`2` 未知参数、非法 `--keep`、显式 `--host`；`3` restore 检测到 daemon 存活且未 `--force`。
