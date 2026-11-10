# 排障

## 端口被占

```bash
roamway daemon status          # 看 pid / bind / offline
```

`EADDRINUSE` 时先用 `daemon stop` 释放，或换端口 `--bind 127.0.0.1:8797`。Windows 上 `daemon stop` 用 `taskkill` 并校验端口释放。

## token 401

- 客户端缺 token：加 `--token T`，或设 `ROAMWAY_TOKEN`，或写入 `<data>/token`。
- 多 token 模式下 token 只存 sha256（`tokens.json`），看不到明文；重新 `token create` 会返回一次明文，抄下来后即不可再读。
- scope 不足是 `403`（`read`/`run`/`admin`，admin 含全部），不是 401。

## hosts 退避

`host ls --probe` 或 `host probe <name>` 看状态。探测失败后按退避窗口重试，窗口内再探测返回 `skipped`；网络恢复后自动转回 `online`。

## GBK 乱码

仓库内所有 `.rs`/`.md`/`.toml` 一律 UTF-8 无 BOM、LF 行尾。若终端把中文显示成乱码，先确认文件编码是 UTF-8，不要在 GBK/GB18030 编辑器里保存。

## timeout / 卡住

- `run --timeout 1800` 控制单次上限；`schedule create` 支持 `expires-in`。
- `wait <id> --timeout 60` 等待进入 idle，超时非零退出。
- `api:*` 的 run 无法中断（直连 API），`POST /v1/runs/{id}/stop` 返回 409；`queued` 态仍可取消。

## state.json / logs 膨胀

长期无人值守时先看 `/metrics` 三个字节类 gauge：

```bash
curl -s -H "Authorization: Bearer $TOKEN" localhost:8796/metrics | grep -E 'roamway_(state|logs)_'
```

- `roamway_state_bytes`：`state.json` 文件大小（不存在则 0）。
- `roamway_logs_bytes`：`<data>/logs` 下 `*.log` 合计字节（不递归子目录）。
- `roamway_logs_count`：`<data>/logs` 下 `*.log` 文件数。

需要手动回收时先跑 dry-run 看统计，再实际修剪：

```bash
roamway prune --dry-run -o json   # 只统计不改 state、不删日志
roamway prune                     # 按配置文件里的 keep_runs 修剪并落盘
roamway prune --keep 5            # 本次临时覆盖阈值，不改配置文件
```

`prune` 只删 terminal（done/failed/timeout/killed/interrupted）run 及其日志，绝不删 agent、worktree、schedule、job、approval。

## 状态损坏 / 回滚

- 定期快照：`roamway backup --keep 7`（默认 `--out <data>/backups`），按目录名保留最近 7 个备份目录。
- 回滚：先 `roamway daemon stop`，再 `roamway restore --from <data>/backups/roamway-backup-*`；daemon 仍在跑时先收到 exit 3，确认安全后可 `--force`。
- `restore` 会先把现有 `state.json` 轮换为 `state.json.pre-restore-*`，再从备份目录安装 `config.json`/`schedules.json`/`state.json`；备份源目录不会被删除。
- daemon 活体判定按三种证据短路：`<data>/roamway.pid` 对应的进程存活 → `pid ...`；`<data>/heartbeat` 60 秒内有更新 → `heartbeat ...s ago`；`<data>/bind` 指向的端口可连接 → `port ... accepts connections`。任一命中且无 `--force` 时 `restore` 退出码 3 且一个字节都不写。
- `--force` 只跳过停服检查，不是热合并；在活体上强恢复会丢掉 daemon 内存里尚未落盘的运行记录（其下次 save 会覆盖回去），请先 `kill`（或 `roamway daemon stop`）再 `restore`。
- 与 `state.json.bak` 的区别：`state.json.bak` 是最后一次写盘前的上一版，会被下一次写覆盖；`roamway-backup-*` 是显式快照，可长期保留。

## 帮助与退出码

```bash
roamway help <cmd>        # 与 <cmd> --help 同文本
```

退出码：`0` 成功，`1` 网络/远端失败，`2` 未知命令/未知子命令/缺必填参数/不支持的命令（如 `project`）。