#!/usr/bin/env pwsh
# Windows end-to-end smoke: start a throwaway daemon, run a shell job and a scheduled
# job, assert auth/stop semantics, then clean up.
# Mirrors scripts/smoke.sh step by step; both the CI windows job (3-platform gate) and
# the Release package self-check run this file.
# Usage: pwsh scripts/smoke.ps1 [-Bin ./target/release/roamway.exe] [-Port 18800]
#
# NOTE: this file is UTF-8 WITHOUT BOM and every comment is ASCII on purpose.
# Windows PowerShell 5.1 decodes a BOM-less script as CP936 on a zh-CN host, so a
# non-ASCII comment shifts bytes and the parser then fails with a bogus "unexpected
# token '}'". Keeping the script ASCII-only makes it work under both 5.1 and 7.
[CmdletBinding()]
param(
    [string]$Bin = "./target/release/roamway.exe",
    [int]$Port = 0
)

$ErrorActionPreference = "Stop"
# Native commands must not throw on a non-zero exit: we assert exit codes ourselves.
# Otherwise any non-zero rc (outside Invoke-WebRequest) turns into an unrelated
# PowerShell exception inside the error handler and hides the real FAIL reason.
$PSNativeCommandUseErrorActionPreference = $false

$script:Daemon = $null
$script:Dir = $null

# Return the HTTP status code, working on both Windows PowerShell 5.1 and 7:
# 5.1 lacks -SkipHttpErrorCheck so 4xx/5xx throw; on 7 Response.StatusCode is an enum.
# Both paths normalize to int, so callers only compare numbers and never depend on a
# parameter that exists in just one version.
function Get-Status([string]$Uri, $Headers) {
    $p = @{ Uri = $Uri; TimeoutSec = 5; UseBasicParsing = $true }
    if ($Headers) { $p.Headers = $Headers }
    try {
        $r = Invoke-WebRequest @p
        return [int]$r.StatusCode
    } catch {
        $resp = $_.Exception.Response
        if ($resp -and $resp.StatusCode) { return [int]$resp.StatusCode }
        return 0   # connection refused / timeout: daemon is not up yet
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

# Random port: concurrent CI jobs on the same runner image must not collide.
if ($Port -le 0) { $Port = Get-Random -Minimum 18800 -Maximum 19800 }
$Tok = "smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 16)
$Dir = Join-Path $env:TEMP ("roamway-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$script:Dir = $Dir
$Base = "http://127.0.0.1:$Port"
$Beats = Join-Path $Dir "beats.txt"

# Start-Process does not reliably quote an -ArgumentList array (a path with spaces gets
# split), so build one quoted command line; both the data dir and the binary live under
# a space-free temp path.
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

# 1) /health is public, every other endpoint enforces auth
$code = Get-Status "$Base/health" $null
if ($code -ne 200) { Fail "/health should be public (got $code)" }
$code = Get-Status "$Base/v1/runs" $null
if ($code -ne 401) { Fail "auth not enforced (got $code)" }

# 2) shell agent runs to completion in the foreground
$out = & $BinPath run --host $Base --token $Tok --agent shell --title smoke "echo roamway-ok" 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) { Fail "run exited $LASTEXITCODE" }
if ($out -notmatch "done") { Fail "shell run not done: $out" }

# 3) scheduler: an --every 2s job must fire at least twice within 6s (cmd.exe branch on Windows)
& $BinPath jobs-add --host $Base --token $Tok --agent shell --every 2 --title beat "echo tick>> $Beats" | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "jobs-add failed (rc=$LASTEXITCODE)" }
Start-Sleep -Seconds 6
$n = 0
if (Test-Path -LiteralPath $Beats) { $n = @(Get-Content -LiteralPath $Beats).Count }
if ($n -lt 2) { Fail "scheduler did not fire (beats=$n)" }

# 4) stop: a long run must be interruptible and land in "killed"
$id = (& $BinPath run --host $Base --token $Tok --agent shell --no-wait "ping -n 30 127.0.0.1 >nul" 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($id)) { Fail "run --no-wait produced no id (rc=$LASTEXITCODE)" }
Start-Sleep -Seconds 1
& $BinPath stop --host $Base --token $Tok $id | Out-Null
if ($LASTEXITCODE -ne 0) { Fail "stop failed (rc=$LASTEXITCODE)" }
# After taskkill /T the runner thread writes the status back, which lands on disk slower
# than on unix; poll for up to 10s instead of sleeping 1s once. The assertion itself is
# not relaxed: we still require "killed".
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
