# P9 SPEC — Hub 与事件触发

目标：补齐 Paseo Hub 面（团队/项目/权限/触发器）与 webhook 事件触发。判据同前：NAS 构建 0 warning、单测全绿、行为实测。

依赖约束：std + serde + serde_json + ureq + libc；不引入新框架；不破坏 P1–P8。

## 1. 数据模型

`hub.json`（0600）：`{ "hub_id": "hub_<8hex>", "name": "...", "created_at": "...", "projects": [], "permissions": [], "triggers": [] }`

- `permissions[]`：`{ "subject": "user:alice" | "token:<sha256前缀>", "scope": "read"|"run"|"admin", "granted_at": "..." }`
- `triggers[]`：`{ "id": "trg_<8hex>", "source": "github"|"generic", "repo": "owner/name"|null, "events": ["push","pull_request"], "secret_env": "ROAMWAY_HOOK_<ID>", "agent_spec": { "provider": "...", "cwd": "...", "prompt": "..." }, "enabled": true }`

`.paseo/organization.json`（若存在）中的 triggers 在 `hub deploy` 时导入，重复 id 覆盖。

## 2. CLI（8 条）

- `hub` — 无子命令时打印 hub 概览（id/name/项目数/权限数/触发器数与启用状态）
- `hub init [--name <n>]` — 生成 hub.json（已存在则拒绝，`--force` 覆盖）；打印 hub_id 与下一步命令
- `hub permissions [ls|grant <subject> <scope>|revoke <subject>]` — 默认 ls；scope 仅接受 read/run/admin，非法值 400 风格退出码 2
- `hub projects [ls|add <path>|rm <path>]` — 项目是本地目录注册（存在性校验），重复添加幂等
- `hub deploy [--file .paseo/organization.json]` — 导入/更新 triggers，输出 `imported N, updated M, skipped K`
- `hub logout` — 删除本地 hub 凭据与 hub.json（`--keep-data` 仅清凭据）
- `hub status` — 守护进程内的 hub 端点状态、已注册 webhook 路径、最近 5 次触发（时间/来源/结果）
- `hub token rotate` — 轮换 hub 级 token（写 0600，旧 token 立即失效）

## 3. HTTP（`src/hub.rs` + server 路由）

- `GET  /v1/hub` — 概览（scope: read）
- `POST /v1/hub/hooks/{trigger_id}` — webhook 入口：校验 `X-Hub-Signature-256`（HMAC-SHA256，密钥取 env `ROAMWAY_HOOK_<ID>`；无密钥则拒绝 403）；解析 GitHub 事件（`X-GitHub-Event`），按 triggers 的 events 过滤；命中则创建 agent + run 返回 `202 {run_id}`，重复 delivery（`X-GitHub-Delivery` 已见）返回 `200 {duplicate:true}`
- `GET  /v1/hub/triggers` / `POST /v1/hub/triggers/{id}/enable|disable`（scope: admin）
- 鉴权沿用 P6 scope 体系：read/run/admin 分级，`hub deploy` 与 enable/disable 需 admin

## 4. 安全与稳健

- HMAC 用常量时间比较；body 上限沿用 1 MiB；签名失败不落原文，只记 id + 来源 IP 掩码
- webhook 触发的 run 打 `trigger_id` 标签，`ls --trigger` 可筛
- 触发器创建 agent 时若无空闲并发槽，返回 `429` 并记 `skipped:busy`，不排队堆积

## 5. 验收（NAS 实测）

1. 单测 ≥10：hub.json 生命周期、permissions scope 校验、triggers 导入合并、HMAC 校验（对/错/缺）、duplicate delivery、429 busy
2. 起临时实例（动态端口）：`hub init` → `hub projects add` → `hub deploy` → 用 `openssl`/python 生成 HMAC 打 `POST /v1/hub/hooks/<id>` → 断言 202 + run 真被创建并完成
3. 错签名 → 403；重复 delivery → 200 duplicate；无 admin scope 调 enable → 403
4. `hub logout` 后 `hub` 返回未初始化提示，退出码 2
