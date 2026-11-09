# 插件（plugin）

可信、非沙箱插件；协议为「子进程 + 一行 JSON 进出」，安全边界清楚。

## 命令

```bash
roamway plugin init <name>              # 生成插件骨架（未启用）
roamway plugin ls                       # 列出插件（NAME/VERSION/ENABLED/TRUSTED/TIMEOUT/ENTRY）
roamway plugin enable <name> --trust    # 启用；--trust 标记为可信
roamway plugin disable <name>
roamway plugin reload <name>            # 重载
roamway plugin logs <name>              # 看插件输出
roamway plugin remove <name> [--keep-files]   # 移除配置（默认连文件一起删）
```

## 生命周期钩子

插件通过 stdin 收到一行 JSON：`{"hook": "<name>", "payload": {...}}`，stdout 回一行 JSON，例如 `{"ok": true, "message": "..."}`。现有钩子：

| 钩子 | 触发时机 |
|---|---|
| `agent.created` | 创建 agent/run 时 |
| `agent.finished` | agent 结束时 |
| `run.failed` | run 失败时 |

## 约束

- `--trust` 是启用受信插件（可执行任意本机命令）的显式开关，不默认开。
- 插件默认有超时（timeout_s），超时按失败记录到插件日志。
- 挂件输出落 `<data>/plugins/<name>/` 下，`plugin logs` 读取。