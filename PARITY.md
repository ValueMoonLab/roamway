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
| 3 | `paseo run` | ⚠️ | `src/main.rs:fn run_plan` / 测试：`run_plan_worktree_mode_and_unsupported_validation` | agent-backed，支持 worktree/cwd/env/label；不支持 `--thinking/--mode/--image/--output-schema/--provider`（用 `--agent`）与 `--worktree-slug/--new-branch/--base/--branch`，均 exit 2 |
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
| 15 | `paseo daemon run` | ✅ | `src/main.rs:fn cmd_daemon_run` | 前台按 `resolve_config` 起生产 serve；`--foreground` 接受（serve 本就前台） |
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
| provider 会话续跑 | ⚠️ | `src/agents.rs:fn should_resume`、`src/runner.rs:fn apply_resume_note`、`src/server.rs:fn agent_session_route`、`src/main.rs:fn resume_target` / 测试：`should_resume_requires_resumable_kind_and_session`、`session_route_hits_session_endpoint_before_agent_route`、`resume_target_parses_agent_and_prompt_and_rejects_conflicts`、`apply_resume_note_marks_done_run_error_without_changing_status` | 首轮续跑 ✅（`run --resume/--continue` + `GET /v1/agents/{id}/session`）；unsupported 显式化 ✅（`run.resume_note="resume unsupported"`）；`--thinking` 等未对齐项仍在 |

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

Paseo 未对齐项：`daemon run` 的 `--thinking/--mode/--provider` 等 agent 参数继续走 `run` 的既有不支持面（exit 2），不在 daemon 配置面内。

P14 新增单测（`src/config.rs`）：`resolve_priority_flag_over_env_over_file_over_default`、`resolve_env_comma_array_and_invalid_values`、`config_roundtrip_and_no_token_leak`、`config_load_bad_json_reports_line`、`config_rejects_forbidden_and_unknown_keys`、`config_key_whitelist`、`apply_set_validates_and_applies`、`trim_runs_keeps_newest_and_never_drops_running`。