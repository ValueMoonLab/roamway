# P8 SPEC — provider / 用量 / 插件 / 语音

目标：把 Paseo 的 provider 生态、账号用量、可信插件、语音四块补齐。判据同前：NAS `cargo build --release` 0 warning、`cargo test` 全绿、CLI 行为实测通过才打 ✅。

依赖约束：只用 std + serde + serde_json + ureq + libc；不引入 tokio/axum/reqwest/clap；不破坏 P1–P7 已有行为。

## 1. provider 抽象（`src/providers.rs`）

统一 `Provider` 描述结构（可序列化，落 `providers.json`）：

```
{ "id": "api:deepseek", "kind": "api" | "agent", "label": "DeepSeek",
  "base_url": "https://api.deepseek.com/v1", "env_key": "ROAMWAY_DEEPSEEK_KEY",
  "models": ["deepseek-v4-pro", "deepseek-v4-flash"], "enabled": true }
```

内置四条：`api:deepseek`、`api:qwen`（dashscope compatible-mode）、`agent:codex`、`agent:claude`。`providers.json` 可覆盖/新增（同 id 合并，用户字段优先）。

- `roamway provider ls` — 表格：id / kind / 状态(ready|no-key|no-binary|disabled) / 模型数 / endpoint（key 只显示 `[set]`/`[missing]`，绝不打印值）
- `roamway provider models [<id>]` — 内置表优先；`--refresh` 时对 api 类发 `GET {base}/models`（ureq，超时 10s），失败保留内置表并写警告到 stderr
- `roamway provider diagnostic <id>` — 逐项输出环境诊断：env key 是否存在、agent 类二进制是否在 PATH（`which` 等价实现）、endpoint 连通性（`GET {base}/models` 仅取状态码）、配置文件路径与权限
- `roamway provider` — 无子命令时等价 `provider ls`

## 2. 用量（`src/usage.rs`）

- `Run` 增加 `usage: { prompt_tokens, completion_tokens, total_tokens, model, provider }`（`#[serde(default)]`，旧 state.json 兼容）
- API 适配器解析响应里的 `usage` 字段落库；agent 类无 usage 时计 0 并记 `"usage":"unavailable"`
- `roamway usage ls [--since 7d|24h|ISO] [--provider id]` — 按 天×provider×model 聚合表：runs / in / out / total
- `roamway usage inspect <run_id>` — 单次调用的模型、token、耗时、是否重试
- `roamway usage [--json]` — 完整报告：总览 + 分 provider + 分模型 + 最近 20 条最贵 run（按 total_tokens 降序）
- 数据源只读 `state.json` 里已完成的 run，不额外落库

## 3. 插件（`src/plugins.rs`）

`plugins.json` 注册表 + 每插件目录 `~/.roamway/plugins/<name>/{plugin.json,run.sh|run.py}`：

```
plugin.json: { "name": "hello", "version": "0.1.0", "entry": ["python3", "main.py"],
               "enabled": true, "trusted": true, "timeout_s": 30 }
```

协议 v1：父进程把 `{"hook":"agent.finished","payload":{...}}` 一行 JSON 写子进程 stdin，子进程回一行 JSON `{"ok":true,"message":"..."}`；超时 kill 进程组。**只允许 trusted 插件运行**，`plugin enable` 对未标 trusted 的拒绝并提示 `--trust`。

- `plugin ls` / `plugin init <name>`（生成骨架 + plugin.json，不启用）/ `plugin logs <name>`（保留最近 256 行环形）/ `plugin reload <name>` / `plugin enable|disable <name> [--trust]` / `plugin remove <name> [--keep-files]`
- 钩子点：`agent.created`、`agent.finished`、`run.failed`；插件失败只写日志不影响主流程
- 单测：注册表读写、trusted 拒绝、超时 kill、日志环形截断、损坏 plugin.json 报错不 panic

## 4. 语音（`src/speech.rs`）

`speech.json`：`{ "stt": ["whisper", "--model", "base", "--output-txt"], "tts": ["edge-tts", "--write-media"] }`（命令模板，`{in}` `{out}` 占位）

- `roamway speech transcribe <file>` — 跑 stt 模板，输出识别文本；未配置则报 `speech: stt not configured` 并给配置示例
- `roamway speech say <text> [--out file]` — 跑 tts 模板，默认输出 `~/.roamway/tts/<ts>.mp3` 并打印路径
- `roamway speech` — 显示当前配置与可用性（二进制是否存在）
- 不做内置语音模型；只做命令编排

## 5. 验收（NAS 实测）

1. `cargo test` 新增 ≥12 条（providers 合并/诊断、usage 聚合、plugin 信任与超时、speech 模板展开）
2. `provider ls/models/diagnostic` 三命令真实输出（deepseek key 存在时 models 走网络，缺 key 时降级）
3. 造 3 条带 usage 的 run → `usage ls` 聚合数字对得上，`usage inspect` 单条对得上
4. `plugin init demo` → `plugin enable demo --trust` → 触发一次 `agent.finished` → `plugin logs demo` 有回执；`plugin enable` 不带 `--trust` 被拒
5. `speech say` 用 `/bin/echo` 冒充模板验证占位替换与产物路径
