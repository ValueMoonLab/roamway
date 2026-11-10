# P20_SPEC — Windows 原生产物 + Windows CI 冒烟（roadmap 第 1、4 项收口）

前置事实（实测于 P16 之后的 HEAD `5a4c872`，锚点为文件:行）：

- `.github/workflows/ci.yml`：matrix `[ubuntu-latest, macos-latest, windows-latest]`；
  步骤 checkout → setup-rust-toolchain(stable) → fmt/check(clippy continue-on-error) → `cargo test`
  → `cargo build --release` → `Smoke test (unix)`（`if: runner.os != 'Windows'`，`run: bash scripts/smoke.sh`）。
  **Windows job 目前只验证编译+单测，没有端到端冒烟。**
- `scripts/smoke.sh` 依赖 curl / mktemp / seq / `&` 后台进程 / kill —— 全部按 unix shell 写死。
- `src/main.rs:2519` shell agent 已按 `cfg!(windows)` 走 `cmd.exe /C`，Windows 上 `run --agent shell` 语义可用。
- `src/main.rs:1079` `roamway version` 打印 `env!("CARGO_PKG_VERSION")`（Cargo.toml:3 = 1.0.0）。
- README.md:404-411 Roadmap 未勾选项：`Windows 原生预编译产物`、`/metrics`（P18 做）、`任务依赖`（P19 做）、`只读 Web 面板`。
- 驱动器发布链要求三平台 CI（探针分支）全绿才可推 main：CI 红了 main 不动，这是本阶段唯一的 Windows 端到端验证通道。

本阶段目标：Windows 产物在 CI 里完成端到端冒烟；打 tag 自动出三平台预编译产物并挂到 GitHub Release；勾选 roadmap 第一项。

---

## 一、`scripts/smoke.ps1`（Windows 端到端冒烟，PowerShell 7 / pwsh）

行为对照 smoke.sh，逐条断言，任何一条失败 `$host.ExitCode=1` 并以 `FAIL: <原因>` 结束：

1. `$BIN` 默认 `./target/release/roamway.exe`（`-Bin` 参数可覆盖，驱动器/CI 用默认）。不存在 → FAIL。
2. 随机端口（18800-19799）+ 随机 token + `$env:TEMP\roamway-smoke-<guid>` 数据目录。
3. `Start-Process -FilePath $BIN -ArgumentList 'serve','--bind',"127.0.0.1:$PORT",'--data',"$DIR\data",'--token',$TOK -RedirectStandardOutput "$DIR\serve.log" -NoNewWindow -PassThru` 起 daemon；
   轮询 `GET /health`（`Invoke-WebRequest -SkipHttpErrorCheck`）最多 50 次×200ms；不起来 → FAIL（打印 serve.log 尾部）。
4. 断言 `GET /health` 无 token = 200；`GET /v1/runs` 无 token = 401（auth 强制）。
5. `& $BIN run --host $BASE --token $TOK --agent shell --title smoke "echo roamway-ok"` 输出含 `done`。
6. `& $BIN jobs-add --host $BASE --token $TOK --agent shell --every 2 --title beat "echo tick>> $DIR\beats.txt"` 成功；
   `Start-Sleep 6` 后 `beats.txt` 行数 ≥ 2（调度器在 Windows 上照常触发）。
7. `run --no-wait "ping -n 30 127.0.0.1 >nul"` 取 ID → `stop` → `status` 输出含 `"killed"`。
8. `Stop-Process -Id $daemon -Force`；清理 `$DIR`；成功打印 `OK: auth, run, scheduler, stop` 且 exit 0。

注意：全部输出 UTF-8 无 BOM；不要用 bash-only 构造；`Invoke-WebRequest` 超时用 `-TimeoutSec 5`。

## 二、`ci.yml` 增加 Windows 冒烟步骤

- 在 `Smoke test (unix)` 之后新增：

```yaml
      - name: Smoke test (windows)
        if: runner.os == 'Windows'
        shell: pwsh
        run: ./scripts/smoke.ps1
```

- 其余步骤不动（`if: runner.os != 'Windows'` 的 unix 冒烟保持原样）。

## 三、`.github/workflows/release.yml`（tag 触发，三平台预编译产物）

- `on: push: tags: ['v*']`；jobs：
  1. `build`：matrix `os: [ubuntu-latest, macos-latest, windows-latest]`，
     `cargo build --release` 后打包：unix → `roamway-<tag>-<os>-<arch>.tar.gz`
     （含 `roamway`、`scripts/smoke.sh`、README.md）；windows → `roamway-<tag>-windows-x64.zip`
     （含 `roamway.exe`、`scripts\smoke.ps1`、README.md）。`actions/upload-artifact@v4` 上传。
  2. `release`：`needs: build`，下载全部 artifact，`softprops/action-gh-release@v2` 或
     `gh release create`（用 `secrets.GITHUB_TOKEN`，不要新增 secrets）建 Release，附改动摘要取 `CHANGELOG.md` 首段。
- 产物名里 target triple 用 `x86_64-unknown-linux-gnu` / `aarch64-apple-darwin` / `x86_64-pc-windows-msvc`
  （macos-latest 是 arm64 runner）。

## 四、README 更新

- Roadmap：`Windows 原生预编译产物` → `[x]`。
- 「安装」节（README.md:70 起）新增「预编译产物」小节：从 GitHub Releases 下载
  `roamway-v*-windows-x64.zip` 解压即用，Windows 冒烟命令 `pwsh scripts/smoke.ps1`。
- 不要动其它章节（尤其 P1-P3 验收过的 CLI 行为文档）。

## 五、测试

- `src/` 无 Rust 代码改动（纯 CI/脚本/文档阶段）——但门禁仍要求仓库可编译、184 测试全绿、零 warning。
- 单测兜底：在 `ci.yml`/`release.yml` 不动的前提下，为 `smoke.ps1` 不需要 Rust 测试；
  若驱动器门禁嫌 `substantive_diff` 不足（本阶段 src/ 零改动属预期），keeper 会在发布前把 phase 直接置 build 走人工绿灯通道，SPEC 作者无需处理。

## 六、验收清单

1. 探针 CI 三平台全绿，其中 windows job 出现 `Smoke test (windows)` 步骤且通过。
2. 推 tag `v1.1.0` 后 Release 页出现三个压缩包且 `roamway.exe --version` 可运行（人工抽查）。
3. README Roadmap 第一项打勾。

## 七、红线

- 不改 `cargo test`/`cargo build --release` 既有步骤；不动 unix 冒烟。
- workflow 里不得出现任何 token 明文、内网 IP、用户名；只用 `secrets.GITHUB_TOKEN`。
- 不得 `git push --force`。
