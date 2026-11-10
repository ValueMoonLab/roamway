#!/usr/bin/env pwsh
# 端到端冒烟（Windows 原生）：起一个临时 daemon，跑 shell 任务与定时任务，验证后清理。
# 与 scripts/smoke.sh 逐条对应；CI 的 windows job（三平台门禁）与 Release 包自检都跑这一份。
# 用法：pwsh scripts/smoke.ps1 [-Bin ./target/release/roamway.exe] [-Port 18800]
[CmdletBinding()]
param(
    [string]$Bin = "./target/release/roamway.exe",
    [int]$Port = 0
)

$ErrorActionPreference = "Stop"
# 原生命令非零退出不抛异常，退出码由我们自己断言 —— 否则 Invoke-WebRequest 之外
# 的任何非零 rc 都会在错误处理里变成一条无关的 PowerShell 异常，掩盖真正的 FAIL 原因。
$PSNativeCommandUseErrorActionPreference = $false

$script:Daemon = $null
$script:Dir = $null

# 取 HTTP 状态码，兼容 Windows PowerShell 5.1 与 PowerShell 7：
# 5.1 没有 -SkipHttpErrorCheck，4xx/5xx 会抛异常；7 里 Response.StatusCode 是枚举。
# 两条路都归一成 int，调用方只比较数字，不再依赖某个版本独有的参数。
function Get-Status([string]$Uri, $Headers) {
    $p = @{ Uri = $Uri; TimeoutSec = 5; UseBasicParsing = $true }
    if ($Headers) { $p.Headers = $Headers }
    try {
        $r = Invoke-WebRequest @p
        return [int]$r.StatusCode
    } catch {
        $resp = $_.Exception.Response
        if ($resp -and $resp.StatusCode) { return [int]$resp.StatusCode }
        return 0   # 连接被拒/超时：daemon 还没起来
    }
}

function Fail([string]$msg) {
    Write-Host "FAIL: $msg"
    if ($script:Dir -and (Test-Path -LiteralPath "$script:Dir\serve.log")) {
        Write-Host "--- serve.log (tail) ---"
        Get-Content -LiteralPath "$script:Dir\serve.log" -Tail 30 -ErrorAction SilentlyContinue | Write-Host
    }
    if ($script:Daemon -and -not $script:Daemon.HasExited) {
        try { Stop-Process -Id $script:Daemon.Id -Force -ErrorAction SilentlyContinue } catch { }
    }
    if ($script:Dir -and (Test-Path -LiteralPath $script:Dir)) {
        Remove-Item -LiteralPath $script:Dir -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit 1
}

if (-not (Test-Path -LiteralPath $Bin)) {
    Fail "binary not found: $Bin (run cargo build --release first)"
}
$BinPath = (Resolve-Path -LiteralPath $Bin).Path

# 随机端口：CI 上多 job 并发跑同一 runner 镜像时不至于撞端口。
if ($Port -le 0) { $Port = Get-Random -Minimum 18800 -Maximum 19800 }
$Tok = "smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 16)
$Dir = Join-Path $env:TEMP ("roamway-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$script:Dir = $Dir
$Base = "http://127.0.0.1:$Port"
$Beats = Join-Path $Dir "beats.txt"

# Start-Process 的 -ArgumentList 传数组时不做可靠转义（路径含空格就会被拆开），
# 这里拼成一条带引号的命令行；数据目录与二进制都在无空格的临时路径下。
$argline = "serve --bind 127.0.0.1:$Port --data `"$Dir\data`" --token `"$Tok`""
$script:Daemon = Start-Process -FilePath $BinPath -ArgumentList $argline `
    -RedirectStandardOutput "$Dir\serve.log" -NoNewWindow -PassThru

$up = $false
for ($i = 0; $i -lt 50; $i++) {
    Start-Sleep -Milliseconds 200
    if ((Get-Status "$Base/health" @{ "x-roamway-token" = $Tok }) -eq 200) { $up = $true; break }
    if ($script:Daemon.HasExited) { Fail "daemon exited early (code $($script:Daemon.ExitCode))" }
}
if (-not $up) { Fail "daemon did not come up on $Base" }

# 1) /health 免鉴权，其余端点强制鉴权
$code = Get-Status "$Base/health" $null
if ($code -ne 200) { Fail "/health should be public (got $code)" }
$code = Get-Status "$Base/v1/runs" $null
if ($code -ne 401) { Fail "auth not enforced (got $code)" }

# 2) shell agent 前台跑通
$out = & $BinPath run --host $Base --token $Tok --agent shell --title smoke "echo roamway-ok" 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { Fail "run exited $LASTEXITCODE" }
if ($out -notmatch "done") { Fail "shell run not done: $out" }

# 3) 调度器：--every 2 秒的任务应在 6 秒内至少触发两次（Windows 上走 cmd.exe 分支）
& $BinPath jobs-add --host $Base --token $Tok --agent shell --every 2 --title beat "echo tick>> $Beats" | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "jobs-add failed (rc=$LASTEXITCODE)" }
Start-Sleep -Seconds 6
$n = 0
if (Test-Path -LiteralPath $Beats) { $n = @(Get-Content -LiteralPath $Beats).Count }
if ($n -lt 2) { Fail "scheduler did not fire (beats=$n)" }

# 4) stop：长任务必须能被打断并落到 killed
$id = (& $BinPath run --host $Base --token $Tok --agent shell --no-wait "ping -n 30 127.0.0.1 >nul" 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($id)) { Fail "run --no-wait produced no id (rc=$LASTEXITCODE)" }
Start-Sleep -Seconds 1
& $BinPath stop --host $Base --token $Tok $id | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "stop failed (rc=$LASTEXITCODE)" }
# Windows 上 taskkill /T 之后 run 状态由 runner 线程回写，落盘比 unix 慢；
# 轮询到 10s 而不是固定睡 1s 再断言一次 —— 断言本身不放宽，仍要求 "killed"。
$st = ""
$killed = $false
for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Milliseconds 500
    $st = & $BinPath status --host $Base --token $Tok $id 2>&1 | Out-String
    if ($st -match '"killed"') { $killed = $true; break }
}
if (-not $killed) { Fail "run not killed: $st" }

Stop-Process -Id $script:Daemon.Id -Force -ErrorAction SilentlyContinue
$script:Daemon = $null
Remove-Item -LiteralPath $Dir -Recurse -Force -ErrorAction SilentlyContinue
$script:Dir = $null

Write-Host "OK: auth, run, scheduler, stop"
exit 0
