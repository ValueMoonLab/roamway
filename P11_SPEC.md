# P11_SPEC — CLI 面收口（Paseo 0.15.0 对齐）+ 文档重写 + v1.0.0

前置事实（本 SPEC 的数字全部来自实测，不是引用旧文档）：

- Paseo 0.15.0 CLI 真实面：**31 个顶层命令**，全局参数 `-o/--format table|json|yaml`、`--json`、`-q/--quiet`、
  `--no-headers`、`--no-color`、`--host`、`--home`；原始帮助文本见 `docs/paseo-cli-surface.md`。
- roamway 二进制（820d8dc 构建）实测结果：
  - `roamway delete|send|start|heartbeat|permit|project ...` → **不报错，打印总帮助并 exit 0**（静默空转，脚本无法判定失败）
  - `roamway workspace setup|create|archive ...` → 打印 `usage: roamway workspace ls | rm`，exit 0
  - `roamway -o json ls` / `roamway --format json ls` → 打印总帮助（短参 `-o` 不被 `parse_flags` 识别）
  - `roamway help ls` → 打印总帮助（无 per-command help）
  - `roamway ls --help` → **真的执行 ls**；`roamway run --help` → 报 `prompt required`（`--help` 没有短路）
  - `roamway bogusxx` → `unknown command: bogusxx` 但 **exit 0**
- 服务端已有但 CLI 未暴露的端点：`POST /v1/agents/{id}/run`（续跑=send）、`DELETE /v1/runs/{id}`（硬删）、
  `DELETE /v1/agents/{id}`（archive）、`GET /v1/approvals`。

本阶段目标：把上面这些真实缺口补上，让 CLI 面对 Paseo 可替代；同时把 PARITY.md / README / docs 写成可验证的现状，
打 v1.0.0。**不要改 P1–P3 已验收的 HTTP/CLI 行为（只做新增与错误路径修正）。**

---

## 一、全局参数与退出码（src/main.rs，硬性）

1. `parse_flags` 支持短参：`-o <fmt>`（等价 `--format <fmt>`）、`-q`（等价 `--quiet`）、`-h`（`--help`）、`-V`（`--version`）。
   短参只认白名单，未识别的 `-x` 视为位置参数（保持现有行为），不得崩溃。
2. 新增 `--format <table|json|yaml>`；`--json` 等价 `--format json`（两者输出必须逐字节一致）。
   - `yaml` 用自带的极简序列化器把 `serde_json::Value` 输出成 YAML（map/seq/标量/多行串），
     **不得新增 crate 依赖**（NAS 构建无 registry 保障）。
3. `-q/--quiet`：列表类命令只输出 ID（每行一个）；`--no-headers`：表格省略表头；
   `--no-color`：接受并忽略（当前无颜色输出，但不得报未知参数）。
4. `--home <path>` 映射到现有 `--data <dir>`。
5. **退出码契约**（这是本阶段最重要的行为修正）：
   - 未知顶层命令 → stderr `unknown command: X` + 提示 `roamway help`，**exit 2**
   - 已知命令的未知子命令 → stderr `usage: ...`，**exit 2**（现在统一 exit 0，属缺陷）
   - 缺少必填参数 → stderr，**exit 2**
   - 远端/网络失败 → **exit 1**（沿用现有失败分支，改成显式 `std::process::exit(1)`）
   - 成功 → 0
6. `--help`/`-h` 必须在**任何副作用之前**短路：`roamway run --help`、`roamway ls --help` 只打帮助、不建 run、不查列表。
7. `roamway help [cmd]` 与 `roamway <cmd> --help` 输出同一份文本。

## 二、命令补齐（src/main.rs 为主，必要时加 server 路由）

| 命令 | 实现方式（不得凭空造） |
|---|---|
| `start` | = `daemon start`（同一函数，不复制逻辑） |
| `delete [id]` | 运行中先 `POST /v1/runs/{id}/stop`，再 `DELETE /v1/runs/{id}`；输出 `deleted <id>`；`--yes` 可选跳过确认 |
| `send <id> <prompt...>` | `POST /v1/agents/{id}/run {"prompt": ...}`（服务端已有 `continue_agent_route`） |
| `permit ls` | = `GET /v1/approvals`；`permit approve <id>` / `permit deny <id>` 复用现有 approve/deny 函数 |
| `heartbeat ls/create/update/delete` | 薄封装到 schedules：`--agent <id>` 绑定 `agent`，`--cron` / `--every` 传 cron/every；`ls` 按 agent 过滤。文档必须写明「与 Paseo 的差异：roamway heartbeat 落在 schedule 存储」 |
| `workspace create <name> --path DIR` | 新增服务端 `POST /v1/workspaces`（注册已有目录，不 clone）；CLI 调它 |
| `workspace archive <name>` | 复用 `DELETE /v1/workspaces/{name}`（已存在），CLI 侧加别名与措辞 |
| `workspace setup <name>` | 执行工作区注册的 setup 脚本（`scripts` 里同名脚本优先），无则 `no setup script for <name>`，exit 2 |
| `project ls/delete` | **不实现**：roamway 无 project 模型。打印 `project: not supported (no project model; use workspace ls)` 并 **exit 2**，PARITY.md 记入「不做」列 |

同时把这些命令加进 `usage()`（总帮助按「核心 / 运维 / 生态」分组，保持现有中文风格）与 per-command help 表。

## 三、单测（cargo test，必须新增，不能只靠手测）

在 `src/main.rs` 的 `#[cfg(test)]` 里至少覆盖：

1. `parse_flags` 短参：`["-o","json","ls"]` → flags.format=json, pos=[ls]；`["-q","ls"]` → quiet=true。
2. format 决议：`--json` 与 `-o json` 走同一分支产出相同字符串。
3. YAML 序列化：嵌套 `{"a":{"b":[1,"x"]}}` 的 YAML 输出可被 `grep` 到 `a:` / `b:` / `- 1`。
4. 退出码：用子进程或把 dispatch 抽成返回 `Result<(), i32>` 的纯函数后断言未知命令 = 2，已知子命令缺参 = 2。
   （推荐后者：`fn dispatch(cmd,flags,pos) -> i32`，`main` 只负责 `exit(dispatch(...))`，这样可被单测直接调用。）
5. `delete`/`send` 的请求体构造：抽出 `fn delete_plan(id, running) -> Vec<(method, path)>` 之类的纯函数并断言顺序为 stop→delete。

## 四、文档（与代码同一提交）

1. **重写 `PARITY.md`**：31 顶层 / 96 叶子逐条，四列必填：`状态`（✅/⚠️/❌）、`证据`、`差异或"不做"理由`。
   证据只认三种：`src/<file>.rs:fn <name>`、`实测：<命令> → <关键输出>`、`测试：<test 名>`。
   禁止保留 P1 时代的汇总数字；文末「阶段排期」表改为 P1..P11 的完成事实。
2. **README**：新增「Web 控制面」章节（`/ui/`、`/ui/app.js`、token 用法、截图占位说明）；
   命令一览改为 `roamway help <cmd>` 指向，不再手工维护 60 行清单。
3. **`docs/`** 新增：`quickstart.md`（5 分钟：serve → onboard → run → ls → logs）、`config.md`（`~/.roamway/` 数据目录、
   hosts.json、token 文件、环境变量 ROAMWAY_*）、`providers.md`（provider ls/models/diagnostic + 国内模型）、
   `plugins.md`（init/enable/--trust/hooks 生命周期）、`terminals.md`（受管终端无 TTY 的限制）、
   `webui.md`、`troubleshooting.md`（端口占用、token 401、hosts 退避、GBK 乱码、timeout）。
4. `CHANGELOG.md`：`## 1.0.0` 段落，按 Added/Changed/Fixed 列 P4..P11 的用户可见变化。
5. `Cargo.toml` 版本 `0.1.0` → `1.0.0`（`roamway --version` 与 `/health` 的 version 同步）。
6. 打 tag 由 keeper 在发布后执行，codex **不要** push tag。

## 五、验收（keeper 会逐条实测，少一条算失败）

- NAS：`source ~/.cargo/env && cargo build --release && cargo test` 全绿，`WARN_COUNT=0`（`-W unused` 也算警告）。
- 三条新行为实测：
  - `roamway bogus` → rc=2；`roamway workspace setup nope` → rc=2
  - `roamway ls --help` 不产生网络/文件副作用且退出 0；`roamway help delete` 与 `roamway delete --help` 文本一致
  - `roamway --json ls` 与 `roamway -o json ls` 输出一致
- `roamway send/delete` 在真实 daemon 上打一遍（NAS 8796，只读数据用测试 run），`delete` 后 `ls` 里不再出现该 id。
- PARITY.md 里每行的证据列都能被 `grep` 命中真实代码或真实命令。
