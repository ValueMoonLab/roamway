# P25 — 探活可分性 + 版本定版 + usage 面收口

## 背景与已勘事实（实现前请自行复核行号）

P24 上线后，生产冒烟已通过（`health`/`ls`/`run` 均不带 `--host`，rc=0，421 消失），但仍有四处
"对外契约"没对齐：

1. **探活只有两条，且语义混在一起**：`(GET /health)` → `fn health`（公开），`(GET /v1/health)` →
   `fn v1_health`（需 token、字段更全），见 `src/server.rs:1394`–`1395`。没有 `/healthz`、没有
   `/readyz`，容器/systemd/k8s 探针要么去猜 `/health` 的语义，要么被迫带 token 才能拿到完整信息。
2. **liveness 会被 state 锁拖死**：`src/server.rs:2524 fn health` 第一行就是 `app.store.lock()`。
   state 锁被一次大 `state.json` 的 `to_string_pretty` + `sync_all`（或写路径）持有时，`/health`
   跟着挂起；探针把"忙"读成"死"，会触发无谓重启。
3. **版本与 CHANGELOG 对不上**：`Cargo.toml` 仍是 `1.1.0`，但 P21–P24 已上线；`CHANGELOG.md` 顶部是
   `## Unreleased`，而 `.github/workflows/release.yml` 取本文件第一个 `## ` 段作为 Release notes
   —— 现在打 `v*` tag 会把 "Unreleased" 当版本名发出去。
4. **usage 串漏参数**：`backup` 的三处串（帮助头部 `src/main.rs:83`、dispatch `:988`、usage_error
   `:1865`）只写 `[--out DIR] [--keep N]`，但 `cmd_backup` 实际接受 `--data`/`--home`（`:1858` 区域）。
   `restore` 已在收尾轮补齐 `[--data DIR]`，`backup`/`prune` 还没。P24 的教训是：漏写参数会让人
   照着 help 传参 → exit 2 → 误判成功能坏了。

## 目标

让外部探针能区分"活着但忙"与"死了"，让版本/CHANGELOG 可以安全发版，让 usage 文本与实际接受的
参数一致。不新增 crate 依赖，不改 `/health`、`/v1/health` 的既有载荷（P1–P3 已验收行为不动）。

## 一、`/healthz` 与 `/readyz`

- 两条新路由进公开豁免：`needs_auth`（`src/server.rs:994` 那一行）的把关列表加 `/healthz`、`/readyz`。
- `GET /healthz`（liveness）：**不取 store 锁**，只读 `app.started` 与 `env!("CARGO_PKG_VERSION")`，
  返回 `{"ok":true,"version":..,"uptime_s":..}`，200。
- `GET /readyz`（readiness）：用 `app.store.try_lock()`（不是 `lock()`）；拿到锁回
  `{"ok":true,"version":..,"uptime_s":..}` 200，拿不到回 `{"ok":false,"reason":"busy"}` 503。
  忙 ≠ 死，这是本条的全部意义。
- 只允许 GET，其他方法 405；响应走 `respond_full`（保住 JSON 内容类型与安全头）。
- 路由分发点在 `src/server.rs:1394` 那一组 match 臂旁，别把 `/healthz` 塞进 `route()` 的 JSON 路由表
  以外的地方。

## 二、版本定版

- `Cargo.toml`：`version = "1.1.0"` → `"1.2.0"`。
- `CHANGELOG.md`：`## Unreleased` → `## 1.2.0`。
- `roamway --version`、`/health.version`、`/v1/health.version` 一律取 `env!("CARGO_PKG_VERSION")`
  （现状已如此），不得出现第二个硬编码版本常量。
- README Roadmap 勾上本批。

## 三、usage 面收口

- 帮助头部与 `dispatch` 的 usage 串：`backup` 补 `[--data DIR]`；`prune` 若 `cmd_prune` 接受
  `--data`，同样补上（先 grep 确认，别照抄本节）。
- 新增测试：`backup`/`restore`/`prune` 各自的 usage 串必须包含 `--data`（防下次再漏）。
- 顺带核对 `docs/cli.md` 的签名行与 help 输出逐字一致。

## 四、远端 endpoint 是 wildcard 时的边界

- `local_base` 的修复只作用于"不带 `--host`"的本地调用；`--host name` 仍原样使用 `hosts.json` 的
  endpoint（不偷偷改写远端地址）。
- 新增测试固定这个边界：endpoint 写成 `http://0.0.0.0:8796` 时，客户端 base 保持原样。
- `host probe <name>` 在 endpoint 是 wildcard 时，错误文案要点明"endpoint 是 wildcard 地址，
  远端访问请写实际主机名/IP"，避免用户照着 `docs/troubleshooting.md` 的 421 段来回试。

## 验收

- NAS 门禁：`cargo build --release` 无 warning，`cargo test` 全绿（测试数 ≥ 252 + 本阶段新增）。
- 生产冒烟（NAS，**不带 token、不带 `--host`**）：
  - `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8796/healthz` → `200`
  - `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8796/readyz` → `200`
  - `curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8796/metrics` → `401`（鉴权面未被放宽）
  - `roamway --version` → `roamway 1.2.0`；`roamway health` 的 `version` → `1.2.0`
- 三平台 CI 绿（走 `ci-probe-*` 探针分支，红则 main 不被污染）。
- 不变量：`/health`、`/v1/health` 的 JSON 键集与状态码在本阶段前后完全一致。
