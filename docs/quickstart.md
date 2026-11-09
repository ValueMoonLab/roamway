# 快速开始（5 分钟）

目标：在 5 分钟内跑通「起服务 → 首次配置 → 派活 → 看列表 → 看日志」。

## 1. 起服务

```bash
roamway serve --bind 127.0.0.1:8796
```

没有 token 时是开放模式，只允许本机访问。生产/局域网请给 token：

```bash
roamway serve --bind 0.0.0.0:8796 --token mysecret
```

后台运行：

```bash
roamway start                # = daemon start，拉起后写入 pid 与 bind
roamway status               # 看 daemon 状态
```

部署参数可以写进 `config.json`，之后 `daemon start` / `daemon run` 直接复用（部署一次、复现多次）：

```bash
roamway daemon config set bind 127.0.0.1:8796
roamway daemon config set max_concurrent 8
roamway daemon start
```

查看生效值与来源：

```bash
roamway daemon config          # 每行带来源（flag/env/file/default）
roamway daemon config get bind
```

## 2. 首次配置（可选）

```bash
roamway onboard              # 生成 token 与启动指引
```

## 3. 派一个活

```bash
roamway run --agent codex --workspace /path/to/repo "修好失败的测试并说明原因"
```

其他 agent：`hermes`、`claude`、`shell`、`api:deepseek`、`api:qwen`（见 `docs/providers.md`）。

## 4. 看列表与状态

```bash
roamway ls                   # 默认列 agent（ID/STATE/KIND/...）；--runs 看运行记录
roamway status <run-id>
```

## 5. 看日志

```bash
roamway logs <run-id> --tail 200
```

需要帮助时：

```bash
roamway help                 # 总帮助
roamway help run             # 单命令帮助（与 roamway run --help 一致）
```

更多：`docs/config.md`、`docs/webui.md`、`docs/troubleshooting.md`。