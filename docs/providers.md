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

## per-run 参数映射

`run --thinking/--mode/--provider/--image/--output-schema` 与 HTTP 的 `options`/`images` 由 `src/runner.rs:fn provider_args` 统一翻译成 argv，顺序固定 `thinking → mode → provider → images → output_schema`（逐字节固定，便于断言）。

| 选项 | codex | claude |
|---|---|---|
| `thinking=low|medium|high|none` | `-c model_reasoning_effort=<L>` | 环境变量 `MAX_THINKING_TOKENS`（low=2048、medium=8192、high=24576；`none` 不设） |
| `mode=read-only|workspace-write|danger-full-access` | `-s <M>`（无 `mode` 时保持 `-s workspace-write`） | `--permission-mode <M>` |
| `provider=ollama` / `lmstudio` | `--oss --local-provider <P>` | ❌ `run --provider: not supported for agent claude` |
| `provider=oss` | `--oss` | ❌ 同上 |
| `provider=<其它 id>` | `-c model_provider=<P>` | ❌ 同上 |
| `image=<FILE>`（可重复） | `-i <FILE>`，每个一张 | ❌ `run --image: not supported for agent claude` |
| `output_schema=<FILE>` | `--output-schema <FILE>` | ❌ `run --output-schema: not supported for agent claude` |

不支持面汇总：

- `shell`、`hermes`、`api:*`：任何非空 `options` 或 `images` 都直接拒，文案 `run --<k>: not supported for agent <kind>`（exit 2 / HTTP 400），不静默丢弃。
- `claude`：只有 `thinking` 与 `mode` 能透传，其余三项显式拒绝。
- 无选项时 `provider_args(kind, 空, &[], None)` 返回空 Vec，既有 argv 除 `-s` 取值外逐字节不变。

参数取值在 `src/main.rs:fn validate_run_options` 校验（CLI 与 server 各调一次，文案逐字一致）；被拒的参数在派发前判掉，不会出现「跑完才发现没生效」。HTTP 示例：

```bash
curl -s -H "Authorization: Bearer ***" -H 'content-type: application/json' \
  -d '{"prompt":"看一下这张图","agent":"codex","options":{"thinking":"high","mode":"read-only","provider":"ollama"},"images":["/abs/a.png"]}' \
  $BASE/v1/runs
```

## 新增 agent

白名单在 `src/types.rs` 的 `KNOWN_AGENTS`，执行命令映射在 `src/runner.rs` 的 `build_command`；各加一条后重新编译。