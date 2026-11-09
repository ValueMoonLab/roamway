# P15_SPEC — run 参数透传面（thinking / mode / provider / image / output-schema）

前置事实（实测于 P13 之后的 HEAD，锚点为文件:函数）：

- `src/main.rs:246` 的 `run_plan(flags)` 里有一张硬编码不支持清单：
  `thinking, mode, image, output-schema, provider, worktree-slug, new-branch, base, branch` →
  一律 `run --<flag>: not supported by roamway` + exit 2（实测：`roamway run --agent shell --no-wait --thinking high "true"` → rc=2）。
  本阶段移出前 5 个（worktree 4 个留给 P16）。
- `src/runner.rs:fn build_command_with_session` 是唯一的 argv 构造点：codex 走
  `exec --skip-git-repo-check -s workspace-write [-C dir] [--model M] [ROAMWAY_CODEX_ARGS] <prompt>`，
  claude 走 `-p [--resume|--session-id SID] <prompt>`，shell 走 `sh -c <prompt>`，hermes 走 `-z <prompt>`。
  它只读 `Run` 上的字段（`model`/`cwd`/`prompt`/`env`），**没有任何地方能承载 per-run 的 provider 参数**。
- `src/types.rs:129 pub struct Run` 现有字段里没有 options/images/output_schema（`labels`/`env`/`cwd` 是 P12 加的同类先例）。
- codex CLI 真实面（实测 `codex exec --help`）：`-c/--config <key=value>`、`-i/--image <FILE>...`、
  `-m/--model`、`-s/--sandbox {read-only|workspace-write|danger-full-access}`、`-p/--profile`、
  `--oss`、`--local-provider {lmstudio|ollama}`、`--output-schema <FILE>`。
  claude `-p` 只认 `--permission-mode`，没有 `--image` / `--output-schema` / `--provider`。

本阶段目标：把这 5 个参数从"硬拒绝"变成**可透传、有校验、有单测**的显式契约，映射规则逐 provider 写明；
provider 没有对应能力时必须继续显式拒绝（exit 2 / 400），不许静默丢弃。

---

## 一、数据模型（src/types.rs）

1. `Run` 新增两个字段，均 serde default、空则跳过序列化（旧 state.json 可读）：
   - `options: BTreeMap<String, String>`；合法 key 全集：`thinking`、`mode`、`provider`、`output_schema`
   - `images: Vec<String>`
2. 构造点补 `options: BTreeMap::new(), images: Vec::new()`（与 P12 加 `labels` 同一份清单：
   `create_run`、`agents::create_agent_and_run`、`agents::queue_agent_run`、`agents::reload_agent`、
   `schedules.rs` 触发路径、`jobs.rs`、`server.rs`、`config.rs`、`usage.rs` 的测试构造点）。

## 二、校验（src/main.rs + src/server.rs 共用）

1. 新增纯函数 `fn validate_run_options(raw: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String>`：
   - 未知 key → `Err("unknown run option: <k>")`
   - `thinking` ∈ `none|low|medium|high`，否则 `Err("run --thinking must be one of none|low|medium|high")`
   - `mode` ∈ `read-only|workspace-write|danger-full-access`，否则 `Err("run --mode must be one of read-only|workspace-write|danger-full-access")`
   - `provider` 非空且 `[A-Za-z0-9_.-]{1,40}`
   - `output_schema` 必须是**已存在文件**的路径，内容能解析成带 `"type"` 的 JSON 对象，否则
     `Err("run --output-schema: not a JSON object with type: <p>")`
2. CLI：`run_plan` 从硬拒绝清单里移出 `thinking/mode/image/output-schema/provider`；
   `--image` 可重复（`flag_many`），每个值必须是可读文件的绝对/相对路径，否则 exit 2 `run --image: no such file: <p>`。
3. HTTP：`POST /v1/runs`、`POST /v1/agents/{id}/run` 接受可选 `"options": {…}` 与 `"images": [ … ]`；
   校验失败 → 400 `{"error": "<同上文本>"}`。字符串数组以外的类型 → 400 `images must be an array of paths`。
4. 校验在 CLI 与 server 各做一次（CLI 直连 runner 的路径也要挡住），文本保持逐字一致。

## 三、argv 映射（src/runner.rs）

1. 新增纯函数
   `fn provider_args(kind: &str, opts: &BTreeMap<String, String>, images: &[String], mode: Option<&str>) -> Result<Vec<String>, String>`；
   顺序固定 `thinking → mode → provider → images → output_schema`，便于逐字节断言。
2. codex：
   | 选项 | argv |
   |---|---|
   | `thinking=L` | `-c model_reasoning_effort=<L>` |
   | `mode=M` | `-s <M>`（替换现在写死的 `-s workspace-write`，无 `mode` 时保持 `workspace-write`） |
   | `provider=ollama\|lmstudio` | `--oss --local-provider <P>` |
   | `provider=oss` | `--oss` |
   | 其它 provider | `-c model_provider=<P>` |
   | `--image p` | `-i <p>`（每个一张） |
   | `output_schema=p` | `--output-schema <p>` |
3. claude：`thinking` → 环境变量 `MAX_THINKING_TOKENS`（`low=2048`、`medium=8192`、`high=24576`、`none` 不设）；
   `mode` → `--permission-mode <M>`；`provider`/`image`/`output_schema` → `Err("run --provider: not supported for agent claude")` 同构文案。
4. shell / hermes / `api:*`：任何非空 options 或 images → `Err("run --<k>: not supported for agent <kind>")`（exit 2 语义）。
5. 默认行为不变：`provider_args(kind, empty, &[], None)` 必须返回空 Vec，且现有 codex argv 顺序除 `-s` 取值外逐字节不变。
6. 被拒的选项要在 `run` 起始时（派发前）判掉，不留"跑完了才发现参数没生效"的状态。

## 四、可观测

- `GET /v1/runs/{id}` 回显 `options` 与 `images`（有值才出现）。
- `roamway run --json` / `--format json` 的输出里同样出现；表格式不变。
- 至少一处日志行：`[runopts] agent=<kind> keys=<k1,k2> images=<n>`（不打印值，避免泄漏路径以外的敏感内容）。

## 五、单测

1. `provider_args_codex_maps_thinking_mode_provider_image_schema`（逐字节断言 argv）
2. `provider_args_empty_is_identity`（空选项 → `[]`，默认 `-s workspace-write` 保持）
3. `provider_args_claude_rejects_provider_and_image`
4. `provider_args_shell_rejects_any_option`
5. `validate_run_options_rejects_unknown_key_and_bad_values`（`thinking=bogus`、`mode=x`、`output_schema=/nope`）
6. `run_plan_no_longer_rejects_provider_flags`（`--thinking high` 不再进 unsupported 分支，`--worktree-slug` 仍在）

## 六、文档

- `docs/cli.md`：`run` 参数表补 5 项 + 三项校验规则。
- `docs/providers.md`：新增「per-run 参数映射」小节，贴第三节表；写明 claude / shell 的不支持面。
- `PARITY.md`：#3 `paseo run` 的证据补 `src/runner.rs:fn provider_args` 与测试名；worktree 4 个参数未对齐 → 状态保持 ⚠️，等 P16 收口。

## 七、验收（keeper 实测，NAS）

- NAS `cargo build --release && cargo test` 全绿、`WARN_COUNT=0`。
- **不花真实 token 的端到端**：在 NAS 起第二个守护进程（`ROAMWAY_DATA=/tmp/p15data`、`ROAMWAY_BIND=127.0.0.1:8797`、
  `PATH=/tmp/p15shim:$PATH`），`/tmp/p15shim/codex` 是打印自身 argv 到 stdout 的 sh 脚本；
  然后 `roamway run --agent codex --thinking high --mode read-only --image /tmp/p15.png --output-schema /tmp/p15s.json "hi"`
  → `logs` 里出现 `-c model_reasoning_effort=high`、`-s read-only`、`-i /tmp/p15.png`、`--output-schema /tmp/p15s.json`。
- 无选项的 codex run 在同一 shim 下 argv 仍是 `exec --skip-git-repo-check -s workspace-write …`（回归）。
- `roamway run --agent shell --thinking high "true"` → exit 2；`roamway run --thinking bogus "true"` → exit 2 含 `none|low|medium|high`。
- `POST /v1/runs {"options":{"bogus":"1"}}` → 400 含 `unknown run option`。
- 生产 8796 上的既有默认 run 行为冒烟：`roamway run --agent shell "echo p15"` 仍 rc=0。
