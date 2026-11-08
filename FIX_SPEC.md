# roamway 加固修改清单（按顺序逐条实现）

项目：`E:\paseo-lite-rs`（Rust，1712 行，依赖仅 serde / serde_json / ureq）。
目标：把它从"能跑"改成"能长期无人值守，且被 agent 批量调用也不会打爆机器"。
约束：
- 保持现有 API/CLI 兼容（`/v1/runs`、`/v1/jobs`、`roamway run/ls/logs/stop/jobs*`），不要改名。
- 不引入重型依赖。Unix 需要进程组，允许加 `libc`（仅 `cfg(unix)`）。
- 每个修改点都要有对应单测或可验证行为；不要只改注释。
- 不要动 `.env`、不要执行 `git push`、不要改 `.github/workflows` 之外的 CI 配置。

---

## P0（必须修，会出线上事故）

### P0-1 子进程必须按进程组回收（三个独立评审都撞到这一条）

现状：`runner.rs` 用 `Command::new(...).spawn()`，超时/stop/daemon 退出时只 `child.kill()`，杀不到 `sh -c "sleep 30"` 的孙进程，实测留下孤儿跑满 30 秒。

要求：
1. Unix：spawn 前 `.process_group(0)`（`std::os::unix::process::CommandExt`），杀的时候 `libc::kill(-(pid as i32), libc::SIGKILL)`；先 SIGTERM 再 2 秒后 SIGKILL 更好。
2. Windows：`.creation_flags(CREATE_NEW_PROCESS_GROUP)`，杀的时候 `taskkill /T /F /PID <pid>`，且必须带 `CREATE_NO_WINDOW`，不能弹窗。
3. 统一封装成 `fn kill_tree(child: &mut Child)`，`timeout`、`stop`、`Err` 分支全部改用它。
4. `types.rs` 的 `Store::load` 标记 `interrupted` 时，若 run 有 `pid` 且该进程仍在（先按 start_time 或 cmdline 校验，降低 PID 复用误杀），调用同样的 `kill_tree`。

### P0-2 并发上限 + 排队

现状：`server.rs` 每连接 `thread::spawn` 无计数；`POST /v1/runs` 直接起线程，无上限。

要求：
1. 新增 `--max-concurrent`（默认 4），run 超过上限时状态保持 `Queued`，由调度器每轮拾取（`jobs.rs` 已有轮询）。
2. 连接线程加计数上限（默认 64），超限返回 `503` 并关闭，不要阻塞 accept。
3. `/health` 与 `/v1/runs` 返回 `running` / `queued` 计数。

### P0-3 state.json 落盘可靠性

现状：`types.rs` 只 `fs::write`+`rename`；解析失败 `unwrap_or_default()`；`main.rs` 启动后无条件 `save()` → 断电半写会把 jobs 永久清空且无日志。

要求：
1. `save`：写 tmp → `File::sync_all()` → rename → 对父目录 `sync_all()`（Unix）。
2. `load`：文件非空但解析失败 → 先 `rename` 成 `state.json.corrupt-<ts>`，再尝试 `state.json.bak`，都失败才用空状态，并 `eprintln!` 明确报错。
3. 每次成功 save 前，把旧文件 `rename` 成 `state.json.bak`。
4. `main.rs`：仅当 load 成功（含从 bak 恢复）才在启动时 save。

### P0-4 stop 之后任务会"复活"

现状：`server.rs stop_run` 对 `Queued` 直接标 Killed 返回 ok；`runner.rs` 线程醒来无条件 `status = Running`，命令照跑。

要求：
1. `runner.rs` 启动前校验 `status == Queued`，否则直接返回。
2. `stop_run`：无论是否 Queued，都要先插入 cancel 标志（`AtomicBool`），再改状态。
3. 加单测覆盖"stop 之后不会再变 Running"。

### P0-5 鉴权与 agent 白名单

现状：token 缺省仍启动；`runner.rs` 的 `other =>` 把 agent 字段直接当可执行文件名（等于任意命令执行）。

要求：
1. token 为空且 bind 不是 loopback（127.0.0.1/::1）→ 打印错误并 `exit(2)`。
2. agent 只允许：`shell`、`codex`、`hermes`、`claude`、`api:deepseek`、`api:qwen`；其余返回 `400 {"error":"unknown agent"}`。
3. 删掉 query 参数传 token 的分支（`?token=`），只接受 header `x-roamway-token`。

---

## P1（应该修，影响稳定性/安全）

### P1-6 `/health` 免鉴权
`GET /health` 不需要 token，只返回 `{ok, version, uptime_s, running, queued}`；详细计数放 `/v1/health`（需鉴权）。当前实现与 README 不一致，改实现并统一文档。

### P1-7 workspace 必须存在
`POST /v1/runs` 传了 `workspace` 但目录不存在 → `400 {"error":"workspace not found","path":...}`，**禁止**静默回落到 daemon 的 CWD。不传 workspace 时用 daemon CWD，并在 run 详情里记录实际使用的绝对路径。

### P1-8 请求体严格解析
1. 结构体加 `#[serde(deny_unknown_fields)]`，未知字段 → 400（避免 `timeout_secs` 拼错被静默忽略）。
2. `timeout_s` 增加 `#[serde(alias = "timeout_secs")]`。
3. 校验：`timeout_s` 在 1..=21600；`every_secs >= 1`；`max_retries` 语义修正——`0` 表示"不重试"，不是"无限重试"。

### P1-9 accept 出错不能空转
`server.rs` 的 `if let Ok(stream) = incoming.next()` 吞掉 Err，`EMFILE` 时 100% CPU。改为打印错误 + `sleep(50ms)` + continue。

### P1-10 日志截断的 UTF-8 边界
`server.rs` 读日志时 `body[body.len()-200_000..]` 在中文日志上会 panic；改用 `is_char_boundary` 回退。另外把全仓 `lock().unwrap()` 换成 `lock().unwrap_or_else(|e| e.into_inner())`，避免一次 panic 毒化全局锁导致服务永久挂死。

### P1-11 请求头总预算超时
目前只有单次 read 超时（30s），慢速 header 可以长期占线程。加总预算（10s）→ 返回 `408`。

### P1-12 列表与清理
1. `GET /v1/runs?limit=` 限制 1..=200，并支持 `offset`；列表接口不返回 `prompt` 全文。
2. 手动创建的 run 也要走 `prune`（现在只有 job 创建的会清理），避免 state 无限增长。

### P1-13 api:* 任务不可取消
`api:deepseek`/`api:qwen` 的 `_cancel` 未使用，`stop` 却返回 200。二选一：实现分段超时轮询取消标志；做不到就返回 `409 {"error":"api run not cancellable"}`，并在文档写明。

### P1-14 body 超限返回 413
超过 1 MiB 不要截断后当 JSON 解析，直接 `413 {"error":"body too large"}`。

---

## 文档同步（改完必须一起改）

1. `README.md`：`/health` 免鉴权；对比表里"Paseo 无定时任务"已过时（Paseo 0.15 有 cron schedule），改成客观表述；Roadmap 勾掉"并发上限"。
2. `docs/architecture.md`：补一节"进程组与回收"、"并发与排队"、"状态落盘可靠性"。
3. `README.md` 增加 macOS 一节：Intel/Apple Silicon 构建差异、`rsproxy.cn` 镜像（实测安装 24 秒、构建 95 秒）、以及"用户 git 配置里残留失效 http.proxy 会导致 cargo 报 spurious network error，加 `[http] proxy=\"\"` 解决"。

---

## 验收标准（我会在 NAS 上逐条实测）

1. `cargo build --release` 0 warning，`cargo test` 全过。
2. timeout 一个 `sleep 30` 的任务 → 4 秒内判 timeout，且 `ps` 里没有任何残留 sleep。
3. `--max-concurrent 2` 时同时提交 6 个 sleep5 → 只有 2 个 running，其余 queued，总耗时约 15 秒。
4. `stop` 一个刚提交还处于 queued 的任务 → 状态 killed，且命令从未执行。
5. `kill -9` daemon 后重启 → 任务标 interrupted，且旧子进程被回收，不重放。
6. 无 token 启动并 `--bind 0.0.0.0` → 拒绝启动，退出码 2。
7. `workspace=/tmp/nope` → 400。
8. `GET /health` 不带 token → 200；`GET /v1/runs` 不带 token → 401。
9. 中文日志超过 200KB 时 `/v1/runs/{id}/log` 不 panic。
