# P21_SPEC — 只读 Web 看板（roadmap 最后一项）+ PARITY 会话续跑收口

前置事实（实测于 P20 之后的 HEAD `30a362c`，锚点为文件:行）：

- `src/server.rs:321 fn ui_response` 只认三个静态名：`/` → 302 `/ui/`、`/ui/`、`/ui/<name>` →
  `crate::web::asset(name)`，未知名 `UiResponse::NotFound`（P19 已把它从 `failed_total` 里单列）。
  `POST /ui/*` 返回 `None`，落进普通 JSON 路由。
- `src/web.rs:9 fn asset` 映射 `""|index.html` / `app.js` / `style.css`，三者都是
  `include_str!` 内嵌，无构建链、无外部依赖。
- `src/web/app.js:118 fn renderNav` 的导航是硬编码数组
  `[#/agents, #/hosts, #/schedules, #/terminals, #/usage, #/settings]`；
  `:125 fn parseRoute` 走 `location.hash`（`#/<page>/<id>/<sub>`）。
- `src/web/app.js:670` 已经在 Settings 页调 `GET /v1/auth/whoami` 并打印 `d.scopes`，
  **但只用于显示**：所有写按钮（`:150` 新建 Agent、`:372` DELETE agent、`:482` POST hosts、
  Settings 的 `POST /v1/tokens`）对任何 token 都照常渲染。
- `src/auth.rs:200 fn has_scope` + `:205 fn required_scope`：GET 大多要 `read`
  （hosts/schedules/approvals/hub triggers 要 `admin`），写操作要 `run` 或 `admin`。
  即 **read token 在服务端已经是只读的**，缺的只是给它的界面。
- `src/main.rs:3797 fn cmd_ui` 只打印 `/ui/`（`--print` 只打印 URL），没有看板入口。
- `README.md:469` Roadmap 最后一项 `[ ] 只读 Web 面板` 仍未勾；`docs/webui.md` 共 16 行，无看板。
- `PARITY.md:154` 会话续跑行仍是 ⚠️，理由是"`--thinking` 等未对齐项仍在"，而
  `src/main.rs:396` 已把 `thinking/mode/provider/output-schema` 全部透传（P15），该理由已过期。

目标：给 `read` scope 的 token 一个真正只读、可长期挂墙的看板；UI 不再把写按钮亮给没有写权限的 token；
把 PARITY 里过期的 ⚠️ 依据核掉。**不动服务端授权模型，不改 P1–P3 的 CLI/HTTP 行为，不加 crate 依赖。**

---

## 一、`/ui/board` 路由与资产

- `src/server.rs:321 fn ui_response` 增加两条映射（放在 `/ui/` 与通用 `/ui/<name>` 之间）：
  `/ui/board` 与 `/ui/board/` → `UiResponse::Asset(200, "text/html; charset=utf-8", crate::web::BOARD)`。
  `POST /ui/board` 仍返回 `None`（未被 GET 短路）。
- 新增 `src/web/board.html`（含 `<meta charset="utf-8">`、`<link rel="stylesheet" href="/ui/style.css">`
  复用现成样式、`<script src="/ui/board.js">`），`src/web.rs` 增加
  `pub const BOARD: &str = include_str!("web/board.html");` 与配套 `board.js`。
  两个文件都写成 UTF-8（无 BOM）；中文在 HTML 里由 meta charset 声明，不涉及 PowerShell 解码问题。
- `asset()` 增加 `"board.html" => BOARD`、`"board.js" => BOARD_JS`；`""|index.html` 行为不变。

## 二、`board.js` 行为（只读、可挂墙）

1. 启动读 `localStorage['roamway.token']`（与 app.js 同一个键，登录一次两边通用）；
   无 token 或 `/health` 返回 401 → 显示输入框，回车保存后重试，**不清屏**。
2. 轮询 5 秒；`document.hidden` 时暂停，`visibilitychange` 恢复。
3. 只发 GET：`/health`、`/v1/runs?limit=20`、`/v1/agents?limit=20`、`/v1/jobs`、`/metrics`。
   **源码中不得出现 `POST`/`PUT`/`PATCH`/`DELETE` 字面量**（单测断言，防止以后被加成控制台）。
4. 版面：
   - 顶行：`roamway <version>` · online/offline 点 · uptime · 最后刷新时间。
   - 状态计数条：queued / running / done / failed / timeout / killed / interrupted（来自 `/v1/runs` 全量）。
   - 最近 run 表：`ID / AGENT / STATUS / 用时 / TITLE`，running 行高亮；点击行不跳转（看板只读）。
   - jobs 表：`ID / EVERY 或 CRON / 上次状态 / AFTER（依赖）/ 等待原因`（依赖与原因取 P19 的
     `depends_on` 与 `last_skip`，字段缺失时显示 `-`）。
   - `/metrics` 关键 gauge 原样透出（`roamway_runs*`、`roamway_conns` 之类），取不到就显示 `-`。
5. 网络错误：顶栏红点 + 保留上一次成功的数据，绝不把表格清空成空白页。
6. 无任何 `<form>` 提交、无 `location.hash` 跳转 —— 看板是单屏，不参与控制面的 hash 路由。

## 三、控制面 UI 只读自适应（`src/web/app.js`）

- `state.scopes` 缓存 `GET /v1/auth/whoami` 的 `scopes`（登录成功后拉一次，失败按 `[]` 处理）。
- 新增纯函数 `canWrite(scopes)`：数组含 `admin` 或 `run` → `true`，否则 `false`。
- `canWrite === false` 时：
  - Agents 页不渲染「新建 Agent」（`:150`）、详情页不渲染 stop/rm/删除/detach 按钮；
  - Hosts 页不渲染「添加 host」表单；Schedules 页不渲染增删；Settings 页隐藏「生成 Token」卡片，
    改为一行 `只读：当前 token scope 为 read`；
  - 现有只读视图（列表、详情、diff、日志、SSE）保持不变。
- 不改变任何请求的构造方式，也不在前端做安全判断（服务端仍是唯一授权点）。

## 四、CLI

- `roamway ui --board` 打印 `<base>/ui/board`；`ui --print --board` 只打印 URL（脚本可用）。
  无 `--board` 时行为与现在完全一致（`/ui/`）。
- 未知参数按 P17 规则 usage error + exit 2，不得静默忽略。

## 五、文档与 PARITY 收口

- `docs/webui.md`：新增「只读看板」段 —— URL、5 秒轮询、需要的 scope（read 即可）、
  明示看板不发任何写请求；补一句 read token 在控制面看到的按钮是隐藏的。
- `README.md:469` 勾选「只读 Web 面板」；`docs/quickstart.md` 若有 UI 小节，补一行 `roamway ui --board`。
- `PARITY.md:154` 会话续跑行：逐条核对 `--thinking/--mode/--provider/--output-schema/--image`
  在 `src/main.rs:396 fn run_plan` 与 `src/runner.rs:fn provider_args` 的落点，把状态改 ✅ 并写真实证据；
  若仍有 provider 侧不做项，保留 ⚠️ 并在「差异」列写明是哪一项、为什么。

## 六、测试（新增，全部纯函数或静态资产断言）

- `src/web.rs`：`board_assets_are_non_empty`、`board_js_issues_no_mutating_requests`
  （`!BOARD_JS.contains("POST")` 等四条）。
- `src/server.rs`：`ui_board_route_serves_board_html`、`ui_board_trailing_slash`、
  `ui_board_post_is_not_static`。
- `src/main.rs`：`ui_target_switches_board_and_index`（`--board` → `…/ui/board`，否则 `…/ui/`）。
- `src/web/app.js` 的 `canWrite` 无法在 Rust 侧单测，改为在 `src/web.rs` 里断言
  `APP_JS.contains("function canWrite")`（存在性守卫，避免以后被删）。

## 七、验收

1. NAS：`source ~/.cargo/env && cargo build --release && cargo test` 全绿，
   `cargo build --release` 0 warning；测试数 ≥ 212。
2. `bash scripts/smoke.sh` 与 `pwsh scripts/smoke.ps1` 仍绿（本阶段不碰执行路径，不得回归）。
3. 手工（NAS 或 PC 连 NAS daemon）：`serve` 起后
   - 用 `read` token 打开 `/ui/board` → 计数 5 秒刷新，断网后保留旧数据；
   - 用 `read` token 打开 `/ui/` → 看不到新建/删除/添加类按钮，Settings 有只读提示；
   - 用 `admin` token 打开 `/ui/` → 按钮与写操作照旧可用。
4. 三平台 CI 绿（驱动器探针分支）→ commit/push/deploy 由驱动器完成。

## 八、边界

- 不改 `auth::required_scope` 的授权表，不引入新 scope 名。
- 不引入 JS 框架/构建链，`board.js` 与 `app.js` 一样是原生 ES2017、单文件。
- 不把 `/metrics` 变成免鉴权（P18 已定：metrics 走正常鉴权）。
