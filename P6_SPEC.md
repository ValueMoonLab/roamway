# P6 规范：权限与安全（PARITY P6）

现状：单 token、明文比较、无 TLS、无 CORS、无 Host 校验、无审批流。Paseo 有 ECDH+XSalsa20 配对、密码哈希、Unix socket、CORS allowlist、Host 白名单、permit 审批。

P6 目标：把「裸 token 挂公网」升级为**可安全暴露**的面。

## 1. 鉴权模型

| 层 | 规则 |
|---|---|
| token | 常量时间比较（已有）；支持多 token：`<data>/tokens.json` `[{id, name, sha256, created_at, last_used_at, scopes}]`；`--token`/env 仍是单 token 兼容入口 |
| scope | `read`（health/ls/logs/agents 读）、`run`（建 run/agent）、`admin`（hosts/schedules/daemon/stop）；token 默认 `read,run`，`admin` 需显式 `--scope admin` 创建 |
| 密码 | `daemon set-password` 写 `argon2id`（实现放在 `src/auth.rs`，不引 crate：用 `sha2`+固定迭代 PBKDF2-HMAC-SHA256，见 §2） |
| 配对 | `pair` 生成一次性配对码（6 位数字 + 60s 有效），对方提交码换长期 token |

## 2. 密码/配对实现（不新增依赖）

- `pbkdf2(password, salt, iterations=200_000, dklen=32)` 用已有 `sha2`（若无则用 `subtle`,`sha2` 已在依赖里；确认 Cargo.toml，缺则加 `sha2`）
- 存 `password.json`：`{algo:"pbkdf2-sha256", iterations, salt_hex, hash_hex}`，权限 0600
- 登录：`POST /v1/auth/login {"password": "..."}` → 返回短期 token（`scopes=["read","run","admin"]`，TTL 24h，存 `tokens.json` 带 `expires_at`）
- 配对码：内存 + `pairing.json`（fsync），一次性消费；`POST /v1/auth/pair {"code":"123456"}` → 长期 token

## 3. 传输与网络面

- `--tls-cert <pem> --tls-key <pem>`：启用 TLS（用 `rustls` + `rustls-pemfile`；Windows 也可用）。无证书时行为不变（明文 HTTP）
- `--allow-origin <origin>`（可多次）：CORS allowlist；未配置时**不发** `Access-Control-Allow-Origin`；`OPTIONS` 预检正确响应
- `--allow-host <host>`（可多次）：校验 `Host` 头，未配置时允许 loopback + 显式 bind 地址；配置后不匹配返回 **421 Misdirected Request**
- `--unix-socket <path>`：Unix 下监听 Unix socket（`std::os::unix::net::UnixListener`），权限 0600；Windows 编译时必须 `cfg(unix)` 排除
- 所有响应加 `X-Content-Type-Options: nosniff`、`X-Frame-Options: DENY`、`Referrer-Policy: no-referrer`

## 4. 审批流（permit）

- 新增配置 `--require-approval <scopes>`：命中 scope 的请求先入库 `approvals.json`（`{id, ts, method, path, body_hash, token_id, status: pending|approved|denied, decided_at, decided_by}`），返回 **202** + `{"approval_id": "..."}`
- `approve <id>` / `deny <id>` / `approvals [--all]`：本地 CLI 或 `admin` scope token 调用
- 审批通过后，客户端用 `X-Approval: <id>` 重放同一请求，且 body hash 必须一致，否则 **409**
- 审批记录保留 7 天，自动清理

## 5. 命令

| 命令 | 说明 |
|---|---|
| `pair [--json]` | 打印配对码 + 有效期 + 交换命令；`--host` 支持远端 |
| `daemon set-password` | 交互式/`--stdin` 读密码，写哈希 |
| `token create --name N [--scope admin] [--ttl 24h]` | 建 token（明文只显示一次） |
| `token ls` / `token revoke <id>` | 列表（掩码）/吊销 |
| `approve <id>` / `deny <id>` / `approvals` | 审批 |

## 6. 单测（≥12）

常量时间比较、scope 判定矩阵（6 组）、PBKDF2 已知向量、token 过期、配对码一次性、Host 白名单命中/未命中、CORS 预检、审批 body_hash 不符 409、审批过期清理、Unix socket 路径权限、TLS 配置缺失时回退明文。

## 7. 验收（NAS + 云机）

1. 无 token → 401；只读 token 调 `POST /v1/runs` → **403**
2. `token create --scope admin` 后 `host add` 成功；用 read token 调 `host add` → 403
3. `daemon set-password` 后 `POST /v1/auth/login` 拿到 token 并可用
4. `pair` → 另一台机器 `POST /v1/auth/pair` 换到 token；重复用同一码 → 401
5. `Host: evil.com`（配置了 allow-host）→ **421**
6. `--tls-cert/key` 起服务 → `curl --cacert` 成功，明文端口不可用
7. `--require-approval run` → `POST /v1/runs` 得 202；`approvals` 看到 pending；`approve` 后带 `X-Approval` 重放成功；改 body 重放 → 409
8. `cargo build --release` 0 warning、`cargo test` 全过、CI 三平台绿

## 8. 约束

- 不破坏 P1–P5：无 `--tls-*`/`--allow-*` 时行为与现在一致（默认单 token、明文、无 CORS 头）
- 任何 token/密码/配对码不得出现在日志与 `ls` 输出
- 依赖新增仅允许 `sha2`、`rustls`、`rustls-pemfile`
