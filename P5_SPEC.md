# P5 规范：调度、心跳与钩子（PARITY P5，15 条命令）

Paseo 的 `schedule` 有 9 个子命令，roamway 现在只有 `jobs-add --every/--daily`。P5 把调度器补成 cron 级：**cron 表达式 + 时区 + 上限 + 过期 + 暂停/恢复 + 手动触发 + 运行日志**。

## 1. 数据模型（`src/schedules.rs` 新增，落 `schedules.json`）

```rust
pub struct Schedule {
    pub id: String,              // sch_ + 8 hex
    pub name: String,            // 唯一
    pub agent_kind: String,      // shell | codex | claude | api:deepseek | api:qwen
    pub prompt: String,
    pub workspace: String,
    pub model: String,
    pub cron: String,            // 5 段 cron，与 --every 二选一
    pub every_secs: u64,         // 0 表示用 cron
    pub timezone: String,        // "Asia/Shanghai" / "UTC" / 固定偏移 "+08:00"
    pub enabled: bool,
    pub paused: bool,
    pub max_runs: u64,           // 0 = 不限
    pub runs_count: u64,
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub last_status: String,     // ok | failed | timeout | running | ""
    pub consecutive_failures: u32,
    pub overlap: bool,           // 上一次还在跑时是否允许并发（默认 false）
}
```

- 迁移：`jobs.json` 里旧的 `{every|daily}` 记录在首次加载时转成 `Schedule`（`every_secs=86400` + `cron="0 H * * *"`），迁移后写回，旧文件保留为 `jobs.json.migrated`。
- `state.json` 里 `jobs` 字段继续存在（P1 兼容），新调度器以 `schedules.json` 为准。

## 2. cron 求值（`src/cron.rs` 新增，不引第三方 crate）

实现 5 段 `分 时 日 月 周`：`*`、`N`、`a-b`、`a-b/s`、`*/s`、逗号列表。要求：

- `next_after(now: i64, tz_offset_secs: i32) -> Option<i64>`：从 `now` 之后逐分钟求值，**最多向前搜 366 天**，超时返回 `None`
- 支持 `@hourly/@daily/@weekly/@monthly` 别名
- 时区：`Asia/Shanghai` / `UTC` / `+08:00` / `-05:00`；其他名报错并提示改用偏移
- 星期日 = 0 和 7 都接受
- 单测必须覆盖：跨月、跨年、闰日、`*/5`、`1-5`、`0 9 * * 1-5`、DST 无关（固定偏移）

## 3. 命令表

| 命令 | 行为 |
|---|---|
| `schedule create --name N --prompt P (--cron "..." \| --every 30s\|5m\|2h) [--tz] [--max-runs] [--expires-in] [--workspace] [--agent] [--model] [--run-now] [--overlap]` | 建调度并算 `next_run_at` |
| `schedule ls [--all] [--json]` | 表格 ID NAME NEXT RUNS LAST CRON |
| `schedule inspect <id\|name> [--json]` | 全字段 + 最近 5 次运行 |
| `schedule logs <id\|name> [--tail 20]` | 最近运行日志 |
| `schedule pause <id\|name>` / `resume` | 暂停/恢复（恢复时重算 next_run_at，丢弃过期点） |
| `schedule delete <id\|name>` | 删除，运行中的 run 不杀 |
| `schedule run-once <id\|name>` | 立刻起一次，不改变 `next_run_at` |
| `schedule update <id\|name> [--cron --every --prompt --max-runs --tz --enabled]` | 就地更新并重算 |
| `hooks [--json]` | 打印钩子配置与最近触发记录（Paseo 的 hooks 记录 agent 钩子活动） |
| `status [--json] [--host]` | 汇总：版本、uptime、并发、队列、调度数、下次触发、最近失败 |

## 4. 调度器循环

- 每秒 tick：取 `enabled && !paused && next_run_at <= now && (max_runs==0 || runs_count < max_runs) && (expires_at 为 None 或未过)`
- 触发：起 run（复用 P1 runner），`runs_count += 1`，写 `last_run_at/last_status`，`next_run_at = cron.next_after(now)`
- `overlap=false` 且上一次仍 running → 跳过本次，`last_status="skipped(overlap)"`，不累加 runs_count
- 失败退避：连续失败 `n` 次时 `next_run_at = max(cron_next, now + min(60*2^(n-1), 3600))`
- `max_runs` 用尽 → `enabled=false`，保留记录
- 进程重启：从 `schedules.json` 恢复；`next_run_at` 已过期则立即补一次（只补一次，不追赶历史）

## 5. HTTP 路由（新增）

| 方法 | 路径 |
|---|---|
| GET | `/v1/schedules` |
| POST | `/v1/schedules` |
| GET/PATCH/DELETE | `/v1/schedules/{id}` |
| POST | `/v1/schedules/{id}/run` |
| POST | `/v1/schedules/{id}/pause` `/resume` |

## 6. 单测（≥10 个）

cron 求值 6 个（含闰日/跨年/工作日）、`every` 解析、迁移旧 jobs、overlap 跳过、max_runs 停用、失败退避封顶、暂停后不触发、`next_after` 366 天上限。

## 7. 验收（NAS）

1. `schedule create --name t1 --every 5s --prompt "date" --max-runs 3` → 20 秒内跑 3 次后 `enabled=false`
2. `--cron "*/1 * * * *" --tz Asia/Shanghai` → `next_run_at` 是下一分钟整
3. `pause` 后不再触发；`resume` 后恢复
4. `run-once` 立刻起一次且 `next_run_at` 不变
5. 失败场景（prompt 指向不存在命令）→ `consecutive_failures` 递增、退避生效
6. 重启进程后调度继续，过期点只补一次
7. `status` 显示调度数与下次触发时间
8. `cargo build --release` 0 warning、`cargo test` 全过、CI 三平台绿

## 8. 约束

- 不引 cron crate，自己实现并在单测里证明正确性
- 时间统一 i64 秒；时区只做固定偏移换算（不引入 tzdata）
- 所有命令支持 `--json` 与 `--host`
