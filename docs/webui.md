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