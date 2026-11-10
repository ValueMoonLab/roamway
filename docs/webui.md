# Web 控制面

`roamway serve` 启动后浏览器访问 `http://127.0.0.1:8796/ui/`。

- 入口：`roamway ui [--print]` 打印可点开地址；`GET /` 302 到 `/ui/`。
- 静态资源随二进制内嵌：`/ui/`（`index.html`）、`/ui/app.js`、`/ui/style.css`，无需 Node 构建链。
- 登录：页面输入 daemon token，存入 `localStorage`；请求带 `Authorization: Bearer`；401 自动回登录页。
- 页面：Agents 列表/详情与实时输出（SSE）、diff、Hosts 与 probe、Schedules、Terminals、Usage、Settings。
- 移动端：深色模式 + 720px 断点。
- 截图：不随仓库提交二进制截图，界面以源码为准（`src/web/index.html`、`src/web/app.js`）。

示例：

```bash
roamway serve --bind 127.0.0.1:8796 --token mysecret
# 打开 http://127.0.0.1:8796/ui/，输入 mysecret
```
## 只读看板

- 入口：`roamway ui --board [--print]` 打印 `<base>/ui/board`；直接访问 `/ui/board`（尾斜杠 `/ui/board/` 同样可用）。
- 看板只发 GET：`/health`、`/v1/runs?limit=20`、`/v1/agents?limit=20`、`/v1/jobs`、`/metrics`，不发任何写请求。
- 5 秒轮询刷新；页面隐藏时暂停，`visibilitychange` 恢复；网络错误保留上一次成功数据，不清空表格。
- 鉴权要求与 `/metrics` 一致，`read` scope 即可；token 与 `/ui/` 共用 `localStorage['roamway.token']`。
- 同一 `read` token 打开 `/ui/` 控制面时，写按钮（新建 Agent、添加 Host、生成 Token 等）会被隐藏，Settings 显示「只读」提示。
