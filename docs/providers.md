# 提供商（provider）

## 查看

```bash
roamway provider ls                 # 内置 + api provider 状态
roamway provider models <id>        # 某 provider 的模型列表
roamway provider models --refresh   # api provider 拉取远端模型（失败回退内置）
roamway provider diagnostic <id>    # 安装/环境/PATH 诊断
```

## 内置 agent

| agent | 实际执行 | 前置条件 |
|---|---|---|
| `codex` | `codex exec ...` | 装好 codex 并登录 |
| `hermes` | `hermes -z ...` | 装好 hermes |
| `claude` | `claude -p ...` | 装好 claude 并登录 |
| `shell` | `sh -c`（Windows 为 `cmd /C`） | 无 |
| `api:deepseek` | POST DeepSeek `/chat/completions` | `ROAMWAY_DEEPSEEK_KEY` |
| `api:qwen` | POST 阿里云百炼兼容端点 | `ROAMWAY_QWEN_KEY` |

## 国内模型

- DeepSeek：`export ROAMWAY_DEEPSEEK_KEY=sk-...`，然后 `roamway run --agent api:deepseek --model deepseek-chat "..."`。
- Qwen：`export ROAMWAY_QWEN_KEY=sk-...`，然后 `roamway run --agent api:qwen --model qwen-plus "..."`。
- 中转/本地兼容端点：用 `ROAMWAY_DEEPSEEK_URL` / `ROAMWAY_QWEN_URL` 覆盖，例如 Ollama：

```bash
ROAMWAY_DEEPSEEK_KEY=ollama \
ROAMWAY_DEEPSEEK_URL=http://127.0.0.1:11434/v1/chat/completions \
  roamway run --agent api:deepseek --model qwen3:8b "你好"
```

## 会话续跑

- `codex` / `claude` 有 provider 侧会话：`codex` 在 run 结束后从日志提取 `session id:`，`claude` 通过 `claude --help` 探测 `--session-id` / `--resume`。
- `shell` / `api:*` 没有可 resume 的会话 id，续跑时 run 会带 `resume_note: "resume unsupported"`。

```bash
roamway run --resume <agent-id> "继续上次会话"
roamway run --continue <agent-id> "继续上次会话"   # --resume 的别名
```

```bash
curl -s -H "Authorization: Bearer $TOKEN" $BASE/v1/agents/<id>/session
# {"agent_id":"ag_…","kind":"codex","resumable":true,"has_session":true,"session_id":"…","state":"idle","last_error":"","last_run_id":"run_…"}
```

## 新增 agent

白名单在 `src/types.rs` 的 `KNOWN_AGENTS`，执行命令映射在 `src/runner.rs` 的 `build_command`；各加一条后重新编译。