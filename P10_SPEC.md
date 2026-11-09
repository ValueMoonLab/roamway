# P10 SPEC — 控制面 Web UI（完全替代 Paseo 桌面端的最后一块）

目标：交付可在浏览器（桌面 + 手机浏览器）使用的完整控制面，覆盖 Paseo Desktop 的核心操作：看 agent 列表/时间线/改动、操作 agent、看主机与调度、看终端输出、看用量。判据：NAS 构建 0 warning、单测全绿、浏览器实测截图/HTTP 断言通过。

依赖约束：**不引入前端构建链**（无 node/npm/webpack）；静态资源用 `include_str!` 嵌入二进制，随 `serve` 一起发布；后端仍只用 std + serde + ureq。不破坏 P1–P9。

## 1. 静态资源（`src/web/`）

- `index.html` — 单页，无外部 CDN 依赖（内网/国内网络必须可用）
- `app.js` — 原生 ES5/ES2017，无框架；hash 路由 `#/agents`、`#/agents/<id>`、`#/hosts`、`#/schedules`、`#/terminals`、`#/usage`、`#/settings`
- `style.css` — 深浅色跟随 `prefers-color-scheme`，移动端断点 720px
- 路由 `GET /` → 302 到 `/ui/`；`GET /ui/*` 从内存返回（正确 Content-Type、`Cache-Control: no-cache`、CSP `default-src 'self'; style-src 'self' 'unsafe-inline'`）

## 2. UI 功能（对齐 Paseo Desktop）

| 页面 | 必须有的能力 |
|---|---|
| Agents 列表 | 按状态筛选（running/idle/failed/archived）、搜索标题、显示 provider/模型/耗时；30s 自动刷新 + 手动刷新；新建 agent（标题 + prompt + workspace + provider） |
| Agent 详情 | 时间线（消息/工具调用/错误分色）、实时输出（SSE `EventSource`，断线自动重连并带 `Last-Event-ID`）、停止/重跑/归档、复制 run_id、查看 diff |
| diff 视图 | 调 `GET /v1/agents/{id}/diff`（P4 已有 worktree 时返回 unified diff），行级红绿高亮，折叠大文件（>2000 行只显示前 2000 行并提示） |
| Hosts | 列表 + 在线状态点 + `probe` 按钮 + 添加/删除（token 输入框 `type=password`，提交后只显示 `[set]`） |
| Schedules | 列表、启停、下次运行时间、最近 5 次结果 |
| Terminals | 终端列表 + 选中后输出面板（轮询 `capture --since` 增量，2s）；发送按键输入框（危险操作二次确认） |
| Usage | 总览卡片 + 按天/模型表格（复用 P8 `usage` 数据） |
| Settings | token 管理（当前 scope、生成新 token）、provider 概览、主题切换、daemon 版本/uptime |

## 3. 认证与安全

- 登录页输入 token → 存 `localStorage` → 所有 fetch 带 `Authorization: Bearer`；401 自动回登录页
- 前端不渲染任何未转义的服务端字符串（统一 `textContent`，diff 用 `textContent` + CSS 着色，禁止 innerHTML 拼接用户数据）
- SSE 走同源 Cookie/Header 复用；不做 CORS 放宽
- 手机浏览器：viewport meta、触控友好的 44px 最小点击区、长按复制

## 4. CLI

- `roamway agent open <id> [--no-browser]` — 打印 `http://<bind>/ui/#/agents/<id>`，非 `--no-browser` 时尝试系统浏览器（Windows `start`、macOS `open`、Linux `xdg-open`，失败仅警告）
- `roamway ui [--print]` — 打印 UI 入口 URL 与是否需要 token

## 5. 验收（NAS + 浏览器实测）

1. `cargo test` ≥8 条：静态资源嵌入非空、路由 302/200、Content-Type 正确、`/ui/` 无 token 时仍可加载页面但 API 401、CSP 头存在
2. curl 断言：`GET /` → 302 Location `/ui/`；`GET /ui/app.js` → 200 + `application/javascript`；`GET /v1/agents` 无 token → 401
3. 真浏览器（PC Chrome，经 SSH 隧道到 NAS 8796）逐页点：agent 列表有数据、详情 SSE 实时追加、diff 配色、hosts probe、terminals 增量、usage 数字与 CLI 一致；移动端 375×812 无横向滚动
4. 截图存档到 `docs/screenshots/`（脱敏：不含 token、不含内网 IP）
5. README 增补“Web 控制面”章节 + 截图 + 一句话启动方式

## 6. 交付定义（P10 完成即 roamway 可交付）

- 96 条 Paseo CLI 中：✅ 达成 ≥90，剩余项在 PARITY.md 写明差异与原因
- `docs/` 有：快速开始、配置参考、provider 接入（含国内模型）、Hub/webhook、插件协议、终端、Web UI、故障排查
- 三平台 CI 绿（linux/macos/windows），`cargo test` 全绿
- 发布 `v1.0.0` tag + release notes；Dockerfile 可 `docker run` 起 daemon 并打开 UI
