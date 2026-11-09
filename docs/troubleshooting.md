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

## 帮助与退出码

```bash
roamway help <cmd>        # 与 <cmd> --help 同文本
```

退出码：`0` 成功，`1` 网络/远端失败，`2` 未知命令/未知子命令/缺必填参数/不支持的命令（如 `project`）。