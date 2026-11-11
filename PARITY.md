# roamway ↔ Paseo 功能对齐基线

基线：**Paseo 0.15.0**。CLI 真实面为 **31 个顶层命令 + 65 个叶子命令 = 96 行**（`docs/paseo-cli-surface.md` 中 `##` 与 `###` 逐级导出，非旧文档数字）。
目标：roamway 作为 Paseo 的 Rust 实现可直接替代；无法对齐的必须在「差异或不做理由」列写明，不静默跳过。
图例：✅ 已对齐 · ⚠️ 部分对齐 · ❌ 未实现（不做）。

证据只认三种：`src/<file>.rs:fn <name>`（可 grep 命中）、`实测：<命令> → <关键输出>`、`测试：<test 名>`。

## 一、顶层命令（31 条）

| # | Paseo 命令 | 状态 | 证据 | 差异或"不做"理由 |
|---|---|---|---|---|
| 1 | `paseo host` | ✅ | `src/main.rs:fn cmd_host` | 主机注册表 add/ls/rm/enable/disable/probe 全量 |
| 2 | `paseo ls` | ✅ | `src/main.rs:fn cmd_ls` / 测试：`ls_plan_switches_runs_and_global_limit_defaults` | 默认 agent 维度，列 `ID/STATE/KIND/MODEL/TITLE/UPDATED`；`--runs` 看 run 表；`--background` 不做（roamway 无后台列表模型，exit 2） |
| 3 | `paseo run` | ✅ | `src/main.rs:fn run_plan` / `src/runner.rs:fn provider_args` / `src/workspaces.rs:fn worktree_add_from` / 测试：`provider_args_codex_maps_thinking_mode_provider_image_schema`、`run_plan_no_longer_rejects_provider_flags`、`worktree_spec_parses_slug_branch_base`、`worktree_add_from_argv_includes_base`、`run_plan_rejects_slug_without_worktree_workspace` | agent-backed，worktree/cwd/env/label 与每 run 的 `--thinking/--mode/--provider/--image/--output-schema`、`--worktree-slug/--new-branch/--base/--branch` 全部落地；参数集收口于 P15+P16 |
| 4 | `paseo import` | ✅ | `src/main.rs:fn cmd_agent_import` | 导入已有 provider 会话 |
| 5 | `paseo clone` | ✅ | `src/main.rs:fn cmd_clone` | 本地 `workspaces::clone_repo` 或远端 `/v1/workspaces/clone` |
| 6 | `paseo attach` | ✅ | `src/main.rs:fn cmd_agent_attach` | SSE 长连接接输出流 |
| 7 | `paseo logs` | ✅ | `src/main.rs:fn cmd_logs` | `/v1/runs/{id}/log?tail=` |
| 8 | `paseo inspect` | ✅ | `src/main.rs:fn cmd_agent_inspect` | agent 详情 |
| 9 | `paseo wait` | ✅ | `src/main.rs:fn cmd_agent_wait` | 轮询至 idle，支持 --timeout |
| 10 | `paseo archive` | ✅ | `src/main.rs:fn cmd_agent_archive` | 软删除 |
| 11 | `paseo onboard` | ✅ | `src/main.rs:fn cmd_onboard` | 首启 token + 前台/后台/crontab/host 指引 |
| 12 | `paseo start` | ✅ | `src/main.rs:fn cmd_daemon_start` | = daemon start，后台拉起 + pid/bind 落盘 |
| 13 | `paseo hooks` | ✅ | `src/main.rs:fn cmd_hooks` | hook 事件列表 |
| 14 | `paseo status` | ✅ | `src/main.rs:fn cmd_status_summary` | 本机 + `--host` 远端；`status ID` 看单条 |
| 15 | `paseo restart` | ✅ | `src/main.rs:fn cmd_restart` | = daemon restart |
| 16 | `paseo reload` | ✅ | `src/main.rs:fn cmd_reload` | 重读 hosts.json |
| 17 | `paseo pair` | ✅ | `src/main.rs:fn cmd_pair` | 配对信息输出 |
| 18 | `paseo agent` | ✅ | `src/main.rs:fn cmd_agent` | 11 个叶子子命令 |
| 19 | `paseo daemon` | ✅ | `src/main.rs:fn cmd_daemon` | 8 个叶子子命令 |
| 20 | `paseo hub` | ✅ | `src/main.rs:fn cmd_hub` | 6 个叶子子命令 |
| 21 | `paseo terminal` | ✅ | `src/main.rs:fn cmd_terminal` | 5 个叶子子命令 |
| 22 | `paseo script` | ✅ | `src/main.rs:fn cmd_script` | ls/add/rm/start/stop |
| 23 | `paseo schedule` | ✅ | `src/main.rs:fn cmd_schedule` | 9 个叶子子命令 |
| 24 | `paseo heartbeat` | ✅ | `src/main.rs:fn cmd_heartbeat` | 落在 schedule 存储（见叶子表） |
| 25 | `paseo permit` | ✅ | `src/main.rs:fn cmd_permit` | ls/approve/deny |
| 26 | `paseo provider` | ✅ | `src/main.rs:fn cmd_provider` | ls/models/diagnostic |
| 27 | `paseo usage` | ✅ | `src/main.rs:fn cmd_usage` | ls/inspect |
| 28 | `paseo plugin` | ✅ | `src/main.rs:fn cmd_plugin` | 6 个叶子子命令 |
| 29 | `paseo speech` | ✅ | `src/main.rs:fn cmd_speech` | transcribe/say |
| 30 | `paseo project` | ❌ | `src/main.rs:fn cmd_project` | 不做：roamway 无 project 模型，打印 not supported 并 exit 2，请用 workspace ls |
| 31 | `paseo workspace` | ✅ | `src/main.rs:fn cmd_workspace` | ls/create/archive/setup/rm |

## 二、叶子命令（65 条）

| # | Paseo 命令 | 状态 | 证据 | 差异或"不做"理由 |
|---|---|---|---|---|
| 1 | `paseo host ls` | ✅ | `src/main.rs:fn cmd_host_ls` | 注册表 + 探测 + 退避 + token 掩码 |
| 2 | `paseo agent ls` | ✅ | `src/main.rs:fn cmd_agent_ls` | 按 agent 列出 |
| 3 | `paseo agent run` | ✅ | `src/main.rs:fn cmd_agent_run` | 创建 agent 并启动任务 |
| 4 | `paseo agent import` | ✅ | `src/main.rs:fn cmd_agent_import` | 导入 provider 会话 |
| 5 | `paseo agent attach` | ✅ | `src/main.rs:fn cmd_agent_attach` | 接入输出流 |
| 6 | `paseo agent logs` | ✅ | `src/main.rs:fn cmd_agent_logs` | 时间线日志 |
| 7 | `paseo agent open` | ✅ | `src/main.rs:fn cmd_agent_open` | 打开 Web 控制面 |
| 8 | `paseo agent inspect` | ✅ | `src/main.rs:fn cmd_agent_inspect` | 详情：provider/模型/会话/状态 |
| 9 | `paseo agent wait` | ✅ | `src/main.rs:fn cmd_agent_wait` | 等 idle |
| 10 | `paseo agent archive` | ✅ | `src/main.rs:fn cmd_agent_archive` | 软删除 |
| 11 | `paseo agent reload` | ✅ | `src/main.rs:fn cmd_agent_reload` | 重启底层会话 |
| 12 | `paseo agent detach` | ✅ | `src/main.rs:fn cmd_agent_detach` | subagent 转独立 |
| 13 | `paseo agent update` | ✅ | `src/main.rs:fn cmd_agent_update` | 更新元数据 |
| 14 | `paseo daemon start` | ✅ | `src/main.rs:fn cmd_daemon_start` | 后台拉起 + pid/bind |
| 15 | `paseo daemon run` | ✅ | `src/main.rs:fn cmd_daemon_run` / `src/config.rs:fn resolve_config` / 文档：`docs/config.md` | 前台按 `resolve_config` 起生产 serve；`--foreground` 接受（serve 本就前台）；与 `docs/config.md` 的 12 键与优先级逐项一致 |
| 16 | `paseo daemon status` | ✅ | `src/main.rs:fn cmd_daemon_status` | 读 pid/bind，offline 判断 |
| 17 | `paseo daemon stop` | ✅ | `src/main.rs:fn cmd_daemon_stop` | SIGTERM/taskkill + 端口校验 |
| 18 | `paseo daemon restart` | ✅ | `src/main.rs:fn cmd_daemon_restart` | stop+start |
| 19 | `paseo daemon reload` | ✅ | `src/main.rs:fn cmd_daemon_reload` | 重读 hosts.json |
| 20 | `paseo daemon pair` | ✅ | `src/main.rs:fn cmd_daemon_pair` | 配对信息 |
| 21 | `paseo daemon config` | ✅ | `src/config.rs:fn resolve_config` / 测试：`resolve_priority_flag_over_env_over_file_over_default` | 持久 config.json + get/set/unset + 来源列；选项全集见下 |
| 22 | `paseo daemon set-password` | ✅ | `src/main.rs:fn cmd_daemon_set_password` | 密码哈希存储 |
| 23 | `paseo hub init` | ✅ | `src/main.rs:fn cmd_hub_init` | starter Hub |
| 24 | `paseo hub permissions` | ✅ | `src/main.rs:fn cmd_hub_permissions` | ls/grant/revoke |
| 25 | `paseo hub projects` | ✅ | `src/main.rs:fn cmd_hub_projects` | ls/add/rm |
| 26 | `paseo hub deploy` | ✅ | `src/main.rs:fn cmd_hub_deploy` | 部署触发器 |
| 27 | `paseo hub logout` | ✅ | `src/main.rs:fn cmd_hub_logout` | 登出 |
| 28 | `paseo terminal ls` | ✅ | `src/main.rs:fn cmd_terminal` | ls 分支 |
| 29 | `paseo terminal create` | ✅ | `src/main.rs:fn cmd_terminal` | create 分支（无 TTY） |
| 30 | `paseo terminal kill` | ✅ | `src/main.rs:fn cmd_terminal` | kill 分支 |
| 31 | `paseo terminal capture` | ✅ | `src/main.rs:fn cmd_terminal` | capture 分支 |
| 32 | `paseo terminal send-keys` | ✅ | `src/main.rs:fn cmd_terminal` | send-keys 分支 |
| 33 | `paseo script ls` | ✅ | `src/main.rs:fn cmd_script` | ls 分支 |
| 34 | `paseo script start` | ✅ | `src/main.rs:fn cmd_script` | start 分支 |
| 35 | `paseo script stop` | ✅ | `src/main.rs:fn cmd_script` | stop 分支 |
| 36 | `paseo schedule create` | ✅ | `src/main.rs:fn cmd_schedule_create` | cron/every/tz/max-runs/run-now |
| 37 | `paseo schedule ls` | ✅ | `src/main.rs:fn cmd_schedule_ls` | 列表 |
| 38 | `paseo schedule inspect` | ✅ | `src/main.rs:fn cmd_schedule_inspect` | 详情 |
| 39 | `paseo schedule logs` | ✅ | `src/main.rs:fn cmd_schedule_logs` | 调度日志 |
| 40 | `paseo schedule pause` | ✅ | `src/main.rs:fn cmd_schedule_pause` | 暂停 |
| 41 | `paseo schedule resume` | ✅ | `src/main.rs:fn cmd_schedule_resume` | 恢复 |
| 42 | `paseo schedule delete` | ✅ | `src/main.rs:fn cmd_schedule_delete` | 删除 |
| 43 | `paseo schedule run-once` | ✅ | `src/main.rs:fn cmd_schedule_run_once` | 手动触发 |
| 44 | `paseo schedule update` | ✅ | `src/main.rs:fn cmd_schedule_update` | 原地更新 |
| 45 | `paseo heartbeat create` | ✅ | `src/main.rs:fn cmd_heartbeat_create` | 差异：roamway heartbeat 落在 schedule 存储（`--agent` 绑定 agent，`--cron`/`--every` 传 cron/every） |
| 46 | `paseo heartbeat update` | ✅ | `src/main.rs:fn cmd_heartbeat_update` | 差异：同上，PATCH `/v1/schedules/{id}` |
| 47 | `paseo heartbeat delete` | ✅ | `src/main.rs:fn cmd_heartbeat_delete` | 差异：同上，DELETE `/v1/schedules/{id}` |
| 48 | `paseo permit ls` | ✅ | `src/main.rs:fn cmd_approvals` | = GET `/v1/approvals` |
| 49 | `paseo provider ls` | ✅ | `src/main.rs:fn cmd_provider` | ls 分支 |
| 50 | `paseo provider models` | ✅ | `src/main.rs:fn cmd_provider_models` | 含 --refresh |
| 51 | `paseo provider diagnostic` | ✅ | `src/main.rs:fn cmd_provider` | diagnostic 分支 |
| 52 | `paseo usage ls` | ✅ | `src/main.rs:fn cmd_usage` | ls 分支，--since/--provider |
| 53 | `paseo usage inspect` | ✅ | `src/main.rs:fn cmd_usage` | inspect 分支 |
| 54 | `paseo plugin init` | ✅ | `src/main.rs:fn cmd_plugin` | init 分支 |
| 55 | `paseo plugin logs` | ✅ | `src/main.rs:fn cmd_plugin` | logs 分支 |
| 56 | `paseo plugin reload` | ✅ | `src/main.rs:fn cmd_plugin` | reload 分支 |
| 57 | `paseo plugin enable` | ✅ | `src/main.rs:fn cmd_plugin` | enable 分支（--trust） |
| 58 | `paseo plugin disable` | ✅ | `src/main.rs:fn cmd_plugin` | disable 分支 |
| 59 | `paseo plugin remove` | ✅ | `src/main.rs:fn cmd_plugin` | remove 分支（--keep-files） |
| 60 | `paseo project ls` | ❌ | `src/main.rs:fn cmd_project` | 不做：无 project 模型，exit 2 |
| 61 | `paseo project delete` | ❌ | `src/main.rs:fn cmd_project` | 不做：无 project 模型，exit 2 |
| 62 | `paseo workspace setup` | ✅ | `src/main.rs:fn cmd_workspace_setup` | 执行注册脚本，无脚本 exit 2 |
| 63 | `paseo workspace create` | ✅ | `src/main.rs:fn cmd_workspace_create` | 新增 `POST /v1/workspaces` 注册已有目录 |
| 64 | `paseo workspace ls` | ✅ | `src/main.rs:fn cmd_workspace_ls` | 列表 |
| 65 | `paseo workspace archive` | ✅ | `src/main.rs:fn cmd_workspace_archive` | DELETE `/v1/workspaces/{name}` |

## 三、全局参数与退出码（P11 收口）

| 项 | 状态 | 证据 | 说明 |
|---|---|---|---|
| `-o/--format table\|json\|yaml` | ✅ | `src/main.rs:fn parse_flags` | `--json` 等价 `--format json`，输出逐字节一致 |
| `-q/--quiet` | ✅ | `src/main.rs:fn parse_flags` | 列表只输出 ID |
| `--no-headers` | ✅ | `src/main.rs:fn cmd_ls` | 表格省表头 |
| `--no-color` | ✅ | `src/main.rs:fn parse_flags` | 接受并忽略 |
| `--home` | ✅ | `src/main.rs:fn dispatch` | 映射到 `--data` |
| YAML 序列化 | ✅ | `src/main.rs:fn to_yaml` | 自带极简序列化器，无新增 crate |
| `--help/-h` 短路 | ✅ | `src/main.rs:fn dispatch` | 副作用前短路，`help <cmd>` 与 `<cmd> --help` 同文本 |
| 未知命令/缺参 exit 2 | ✅ | `src/main.rs:fn usage_error` | `dispatch` 返回 `FAIL_CODE` |
| 网络失败 exit 1 | ✅ | `src/main.rs:fn request` | 失败分支 `fail(1)` |
| dispatch 纯函数 | ✅ | `src/main.rs:fn dispatch` | `main` 仅 `exit(dispatch(...))` |

## 四、阶段完成事实（P1–P11）

| 阶段 | 交付事实 | 证据 |
|---|---|---|
| P1 | 执行底座：serve/run/ls/status/logs/stop/rm 正确性 | `src/main.rs:fn cmd_run` |
| P2 | 主机联邦与守护进程 | `src/main.rs:fn cmd_host`、`src/main.rs:fn cmd_daemon` |
| P3 | Agent 生命周期与会话 | `src/main.rs:fn cmd_agent` |
| P4 | 工作区（clone/workspace ls/rm） | `src/main.rs:fn cmd_clone`、`src/main.rs:fn cmd_workspace` |
| P5 | 调度（schedule 9 子命令） | `src/main.rs:fn cmd_schedule` |
| P6 | 权限与安全（pair/approve/deny/token/set-password） | `src/main.rs:fn cmd_pair`、`src/main.rs:fn cmd_approve` |
| P7 | 终端与工作区脚本 | `src/main.rs:fn cmd_terminal`、`src/main.rs:fn cmd_script` |
| P8 | 提供商/用量/插件/语音 | `src/main.rs:fn cmd_provider`、`src/main.rs:fn cmd_usage`、`src/main.rs:fn cmd_plugin`、`src/main.rs:fn cmd_speech` |
| P9 | Hub 与事件触发 | `src/main.rs:fn cmd_hub` |
| P10 | Web 控制面 | `src/web.rs:fn asset`、`src/main.rs:fn cmd_ui` |
| P11 | CLI 面收口 + 文档重写 + v1.0.0 | `src/main.rs:fn dispatch`、`src/main.rs:fn usage_error`、`src/main.rs:fn cmd_send`、`src/main.rs:fn cmd_delete`、`src/main.rs:fn cmd_permit`、`src/main.rs:fn cmd_heartbeat` |

新增单测（`src/main.rs` 内 `#[cfg(test)]`）：`parse_flags_short_o_and_q`、`json_flag_and_format_flag_match`、`yaml_serializer_emits_nested_maps_and_sequences`、`dispatch_returns_usage_exit_code`、`delete_plan_stops_running_runs_before_delete`。

P12 新增单测：`parse_labels_accepts_multi_and_rejects_missing_equals`、`run_plan_worktree_mode_and_unsupported_validation`、`ls_plan_switches_runs_and_global_limit_defaults`、`agent_list_url_encodes_labels_and_order`、`parse_flags_short_a_and_g`（`src/main.rs`）；`filter_agents_applies_label_and_kind_and_limit`（`src/agents.rs`）。

## 五、provider 会话续跑（P13）

| 项 | 状态 | 证据 | 说明 |
|---|---|---|---|
| provider 会话续跑 | ✅ | 续跑：`src/agents.rs:fn should_resume`、`src/runner.rs:fn apply_resume_note`、`src/server.rs:fn agent_session_route`、`src/main.rs:fn resume_target`；参数对齐：`src/main.rs:fn run_plan`（`:396` 透传 thinking/mode/provider/output-schema）、`src/runner.rs:fn provider_args`（`:338`）与 `build_command_with_session`（`:486`）；测试：`should_resume_requires_resumable_kind_and_session`、`session_route_hits_session_endpoint_before_agent_route`、`resume_target_parses_agent_and_prompt_and_rejects_conflicts`、`apply_resume_note_marks_done_run_error_without_changing_status`、`provider_args_codex_maps_thinking_mode_provider_image_schema`、`provider_args_claude_rejects_provider_and_image`、`provider_args_shell_rejects_any_option` | 首轮续跑 ✅（`run --resume/--continue` + `GET /v1/agents/{id}/session`）；`--thinking/--mode/--provider/--output-schema/--image` 已对齐：codex 映射 `-c model_reasoning_effort=`、`-s <mode>`、`--oss`/`--local-provider`/`-c model_provider=`、`--output-schema`、`-i`，claude 映射 `--permission-mode` 与 `MAX_THINKING_TOKENS`（provider/image/output_schema 显式报错），shell/hermes/api:* 显式报错不静默丢弃；unsupported 显式化 ✅（`run.resume_note="resume unsupported"`） |

P13 新增单测：`should_resume_requires_resumable_kind_and_session`（`src/agents.rs`）；`apply_resume_note_marks_done_run_error_without_changing_status`、`apply_resume_note_on_non_done_run_keeps_error_untouched`（`src/runner.rs`）；`session_route_hits_session_endpoint_before_agent_route`、`session_route_missing_agent_returns_404`（`src/server.rs`）；`resume_target_parses_agent_and_prompt_and_rejects_conflicts`（`src/main.rs`）。

## 六、P14 daemon 部署面

`daemon config` 选项全集逐条对表（roamway 12 键）：

| roamway key | 状态 | 说明 |
|---|---|---|
| `bind` | ✅ | `src/config.rs:fn resolve_config`；CLI `--bind` > `ROAMWAY_BIND` > file > default |
| `max_concurrent` | ✅ | CLI `--max-concurrent` > `ROAMWAY_MAX_CONCURRENT` > file > default 4 |
| `max_conn` | ✅ | CLI `--max-conn` > `ROAMWAY_MAX_CONN` > file > default 64 |
| `tz_offset_hours` | ✅ | CLI `--tz` > `ROAMWAY_TZ` > file > default 8 |
| `unix_socket` | ✅ | CLI `--unix-socket` > file |
| `tls_cert` | ✅ | CLI `--tls-cert` > file |
| `tls_key` | ✅ | CLI `--tls-key` > file |
| `allow_hosts` | ✅ | CLI `--allow-host`（可重复）> `ROAMWAY_ALLOW_HOSTS`（逗号）> file |
| `allow_origins` | ✅ | CLI `--allow-origin`（可重复）> file |
| `require_approval` | ✅ | CLI `--require-approval`（scope 列表）> `ROAMWAY_REQUIRE_APPROVAL`（bool）> file（bool） |
| `token_env` | ✅ | file；只存环境变量名，token 值永不落盘 |
| `keep_runs` | ✅ | file；0 不修剪，默认 500（roamway 扩展，Paseo 无对应项） |

`daemon run` 转发 `cmd_serve`，按 `src/config.rs:fn resolve_config` 读取 `docs/config.md` 的 12 键与优先级；agent 级参数走 `roamway run`，不在 daemon 配置面内。

P14 新增单测（`src/config.rs`）：`resolve_priority_flag_over_env_over_file_over_default`、`resolve_env_comma_array_and_invalid_values`、`config_roundtrip_and_no_token_leak`、`config_load_bad_json_reports_line`、`config_rejects_forbidden_and_unknown_keys`、`config_key_whitelist`、`apply_set_validates_and_applies`、`trim_runs_keeps_newest_and_never_drops_running`。

## 七、P16 worktree 参数 + 参数面收口

- `--worktree-slug/--new-branch/--base/--branch` 落地于 `src/workspaces.rs:fn spec_from_parts`、`worktree_add_from`、`worktree_path_named` 与 `src/agents.rs:fn create_agent_and_run`；清理按 `run.worktree` 记录反查（`src/agents.rs:fn archive_agent`）。
- 新增单测：`worktree_spec_parses_slug_branch_base`、`worktree_add_from_argv_includes_base`、`worktree_path_named_uses_slug_and_falls_back_to_agent_id`、`worktree_remove_uses_recorded_path`（`src/workspaces.rs`）；`run_plan_rejects_slug_without_worktree_workspace`（`src/main.rs`）。
- PARITY #3 `paseo run` 参数集收口 → ✅；#15 `paseo daemon run` 已对照 `docs/config.md` 核验。
## 八、P17–P22 交付事实（运行/运维面）

| 阶段 | 交付 | 证据（代码锚点） |
|---|---|---|
| P17 | CLI 参数解析硬化：未知短标志一律 usage error，`-i` 别名归一 | `src/main.rs:fn parse_flags`；测试 `parse_flags_rejects_unknown_short_flag` |
| P18 | `GET /metrics`（Prometheus 文本） | `src/server.rs:fn metrics_route`、`respond_static` |
| P19 | job 依赖：依赖未完成则跳过并记原因 | `src/jobs.rs:Job.depends_on`（`jobs.rs:58` 判定）、`jobs-add --after` |
| P20 | Windows 原生产物 + 三平台 CI + Windows 冒烟 | `.github/workflows/release.yml`、`.github/workflows/ci.yml`、`scripts/smoke.ps1` |
| P21 | 只读 Web 看板 + PARITY 会话续跑收口 | `src/server.rs` webui 路由、`GET /v1/agents/{id}/session` |
| P22 | 保留阈值单一真源、日志封顶、`prune` 子命令 + `/v1/prune`、字节 gauge | `src/config.rs:fn trim_runs_plan`（唯一阈值判定）、`src/jobs.rs:fn prune(st, keep_runs)`、`src/runner.rs:fn append_capped`、`POST /v1/prune`、`/metrics` 的 `roamway_state_bytes`/`roamway_logs_bytes`/`roamway_logs_count` |

P22 关键不变量（后续阶段不得回退）：

- 保留阈值只有 `config::trim_runs_plan` 一处判定，`keep_runs=0` 表示不修剪；`jobs`/`schedules`/`POST /v1/runs` 三个调用点都传 `App.keep_runs`，不得再出现硬编码常量。
- `prune` 只删 terminal run 及其 `<data>/logs/<run_id>.log`，绝不删 agent、worktree、schedule、job、approval；`--dry-run` 必须零写入。
- 单 run 日志受 `max_log_bytes`（默认 4 MiB，0=不限）封顶，超限只停写并追加一次 truncate 标记，不重写既有内容。

## 九、P23–P24 交付事实（无人值守纵深 + 运维自证）

| 阶段 | 交付 | 证据（代码锚点） |
|---|---|---|
| P23 | 时间维度保留 `keep_days`（默认 0=不限），与条数阈值共用同一份保留判定 | `src/config.rs:29 CONFIG_KEYS`（14 键）、`src/config.rs:616 fn retention_plan_full`、`src/config.rs:665 fn trim_runs_plan`（薄封装）、`prune --max-age-days N` |
| P23 | `backup` / `restore` 本地快照命令 | `src/backup.rs:205 fn backup`、`src/backup.rs:426 fn restore`、`src/main.rs:fn cmd_backup`、`src/main.rs:fn cmd_restore`；`manifest.json` 逐文件 `bytes`+`sha256` 校验，校验失败零写入 |
| P23 | 写盘去重：内容哈希相同则跳过落盘，并单列跳过计数 | `src/types.rs:fn Store::save`（`last_write_hash` 判定）、`/metrics` 的 `roamway_state_writes_skipped_total` |
| P23 | 数据目录字节 gauge | `/metrics` 的 `roamway_data_bytes`（`<data>` 顶层文件 + `logs/*.log` 求和，`src/server.rs:520` 锁块内取值） |
| P24 | 写计数单一真源：`Store` 内累加，`/metrics` 在已持 store 锁的段内读取 | `src/types.rs:242 pub writes_total`、`src/types.rs:335 self.writes_total += 1`、`src/server.rs:500–521`（锁块内取值后输出） |
| P24 | `<data>/roamway.pid` 由 serve 自己维护（启动写、退出按 owner 校验后删） | `src/backup.rs:278 fn write_pid_file`、`src/backup.rs:293 fn clear_pid_file`、`src/main.rs:1241`（serve 启动处调用） |
| P24 | `restore` 活体判定三证据（pid 进程存活 / heartbeat 60s 内 / bind 端口可连），任一命中即 exit 3 | `src/backup.rs:401 fn daemon_alive`（+ `alive_error_message`）；测试：`daemon_probe_prefers_live_pid`、`daemon_probe_ignores_stale_pid_and_old_heartbeat`、`restore_refuses_when_only_heartbeat_is_fresh`、`restore_refuses_alive_daemon_without_force`、`clear_pid_file_only_when_owner` |
| 收尾 | CLI 421 缺陷：daemon 绑 wildcard（`0.0.0.0`/`::`/`*`/空 host）时，本地无 `--host` 的命令把 `Host: 0.0.0.0` 发给服务端，被 Host 白名单判 421 | `src/hosts.rs:87 fn client_base_from_bind`（+ 私有 `split_authority`）、`src/main.rs:741 fn local_base`；测试 `client_base_maps_wildcard_bind_to_loopback`、`client_base_keeps_concrete_bind`、`local_base_wildcard_bind_becomes_loopback` |

生产实测（NAS，P24 二进制 1.1.0）：

- `roamway health` rc=0、`roamway ls` rc=0、`roamway run --agent shell "echo p25"` → `run_000109 done 2s`，输出 `p25`，exit 0 —— 三命令均**不带 `--host`**，421 已消失。
- `roamway_state_writes_total 6`（daemon 启动后随真实写入增长）、`roamway_state_writes_skipped_total 0`。
- `roamway restore --from <备份> --data <副本>`（不带 `--force`）RC=0，落盘 `state.json` sha256 与 manifest 的 `bytes:37480` 条目逐字节一致。

P24 关键不变量（P22 不变量继续有效，另加以下四条）：

- 写计数只在 `Store::save` 的落盘分支里加，且只在内存态；`Store::load` 必须把 `writes_total`/`writes_skipped` 置 0。禁止再引入 `AtomicU64` 计数，禁止调用点累加。
- `/metrics` 里的计数只能取自 `app.store.lock()` 那个代码块内的局部变量；不得在锁外补读。
- `<data>/roamway.pid` 只允许 `backup::write_pid_file` 写、`backup::clear_pid_file` 删（后者必须校验 owner 是本进程）；其他路径不得直接读写成"重启保护"。
- `daemon_alive` 只可**增加**证据，不可删除既有三条中的任何一条；每条证据都要能被 `restore` 的 exit 3 文案点名（`pid` / `heartbeat` / `port`）。

## 十、P25–P26 交付事实（探活可分性 + 数据目录单写者）

| 阶段 | 交付 | 证据（代码锚点） |
|---|---|---|
| P25 | liveness 与 readiness 分离，且都免鉴权 | `src/server.rs:2532 fn healthz`（纯活体：`ok/version/uptime_s`，恒 200）、`src/server.rs:2541 fn readyz`（`Fresh/Ok/Recovered` → 200 带 `store`，`Unrecoverable` → 503 `state_unreadable`，锁忙 → 503 `busy`）；路由 `src/server.rs:1397–1398`，免鉴权白名单 `src/server.rs:996` |
| P25 | 版本定版 + usage 文本单一真源 | `Cargo.toml:3`（P25 定 1.2.0，P26 起 1.3.0）；`src/main.rs:40 USAGE_PRUNE`、`:42 USAGE_BACKUP`、`:43 USAGE_RESTORE`，引用点只在 `src/main.rs:114–116` 与 `:1002–1003`，不再有裸 usage 字符串 |
| P26 | 数据目录单写者：同 `<data>` 的第二个 `serve` 退出码 3 | `src/backup.rs:301 fn writer_lock_path`、`:306 fn read_writer_lock`、`:318 fn writer_lock_owner`、`:322 fn try_acquire_writer_with`、`:370 fn acquire_writer`、`:375 fn force_acquire_writer`、`:396 fn release_writer`；接线 `src/main.rs:1250`（busy → exit 3）、`:1246`（`--force` 显式接管）、`:1341–1345`（正常退出与启动失败都释放）；文案 `src/backup.rs:403 fn writer_busy_message`、`:412 fn writer_takeover_message` |
| P26 | `StoreHealth` 四值，`/readyz` 追加 `store` | `src/types.rs:236 pub enum StoreHealth { Fresh, Ok, Recovered, Unrecoverable }`（`as_str` 在 `:238`）、`src/types.rs:252 store_health` 字段 |
| P26 | 坏文件隔离快照 + 降级期保住 known-good 备份 | `src/types.rs:403 fn quarantine_corrupt_state`（只留最近 5 份）、`:415 fn recover_from_backup`、`:435 fn preserve_known_good_backup`；`src/types.rs:269–270`（`loaded_ok` / `preserve_backup` 决定首笔 `save()` 是否轮换 `.bak`） |
| P26 | `daemon status` 增 `store` 与 `writer` | `src/main.rs:4296 status["store"] = inspect_store_health(&data)`；`writer` 段同处；文本模式多一行 `store ... writer pid ... bind ...`，daemon 离线也做本地体检 |

P25/P26 关键不变量（P22/P24 不变量继续有效，另加五条）：

- `/healthz` 只回答"进程活着"，不得掺 store 或非关键依赖状态；只有 `/readyz` 允许 503，且 `busy` 不代表该重启。
- `/readyz` 的 `store` 取值集合固定为 `fresh|ok|recovered|unrecoverable`（P26 定）；`recovered` 必须回 200，不得降级成失败。
- writer lock 是 `<data>` 单写者的唯一判据；`acquire_writer` 失败一律 exit 3，不得重试、不得降级成多写者并发。`--force` 必须打印接管警告。
- 降级启动期（`store_health != Ok`）首笔 `save()` 不得轮换 `.bak`，否则会拿坏状态覆盖最后一份已知良好备份。
- `daemon status` 的 `store` / `writer` 只允许新增键；`store` 取值集合不得扩展。

P27 已交付（clippy 门禁转正 + `daemon status` 说真话）：

- clippy 清零：`cargo clippy --all-targets -- -D warnings` 零 error 零 warning；`.github/workflows/ci.yml`
  的 fmt / clippy 两处 `continue-on-error` 已删，clippy 步骤补 `--all-targets`；`#[allow]` 全仓库仅
  `src/agents.rs:188` / `:247`（`too_many_arguments`，注册式 API 参数与协议字段一一对应）与
  `src/speech.rs:32`（`dead_code`，P10 web 控制面板保留的 config writer）。
- `daemon status` 的 bind 单一真源：`src/main.rs:4692 fn status_target`（writer bind 优先 → config →
  默认 `127.0.0.1:8796`），文本输出 `config <file>  effective_bind <b> (<source>)`。
- 活体校验：`src/main.rs:4692` 配合 `src/backup.rs:448/453/478 pub fn process_alive` 三平台实现；pid 已死
  `ok=false`、`uptime_s=0`、`pid=null`、`stale_pid=N`，error 含 `stale pid file (pid N not running)`，
  且绝不再连任何端口。
- 探活地址必须过 `client_base_from_bind`：`src/main.rs:4686 fn status_probe_base`（唯一实现 =
  `hosts::client_base_from_bind`），`:4875` 用它构造探活 base（`183dfa4`）。wildcard 监听
  （`0.0.0.0`/`::`/`*`）当 Host 会被服务端判 421，status 随即误报 `healthz unreachable` 降级成
  `store_source=offline`——即「daemon 明明活着却说自己离线」。
- `store` 取活体真值：`src/main.rs:4756 struct ReadyVerdict`、`:4765 fn readyz_verdict`、
  `:4835 fn cmd_daemon_status`（`GET /readyz` 200 → `store_source=live`；503 `state_unreadable` →
  `unrecoverable`；busy / 401 → 保留离线体检值并 `store_source=offline`）。新键：
  `bind_source` / `stale_pid` / `ready` / `ready_reason` / `store_source` / `store_error`。
- 测试注入 `alive` 纯函数：`src/main.rs:6107`（`status_target_prefers_live_writer_bind_and_source`）、
  `:6121`（config 源）、`:6134`（`status_probe_base_maps_wildcard_bind_to_loopback`）、
  `:6145`（pid 已死离线化）、`:6167` / `:6182`（writer 覆盖陈旧 pid 文件）、
  `:6197` / `:6208`（readyz 判定）、`:6218`（离线 store 与 `inspect_store_health` 一致）。

P27 关键不变量（在 P22/P24/P25/P26 不变量上追加）：

- `daemon status` 只允许新增 `bind_source` / `stale_pid` / `ready` / `ready_reason` /
  `store_source` / `store_error` 六个键；既有键语义不得改，`store` 取值集合仍为
  `fresh|ok|recovered|unrecoverable`。
- pid 文件已死时 status 必须 `ok=false`、`uptime_s=0`、`pid=null`、`stale_pid=N`，并且不得向
  任何端口发起连接（不得回落常量 `127.0.0.1:8796` 拿生产实例的数）。
- status 探活地址只允许经 `status_probe_base` → `hosts::client_base_from_bind` 得到；禁止把
  `effective_bind` 原样当 base（wildcard 会被 421，表现为假离线）。新增探活客户端一律复用这条路径，
  不写第二份 wildcard→loopback 映射。
- `#[allow]` 全仓库上限 3 条且每条带一行理由注释；禁止 `#![allow(...)]`、`-A warnings` 或 CI
  `continue-on-error`。
