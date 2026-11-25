#Requires -Version 7
<#
.SYNOPSIS
    一键校验 CrossRender：格式、lint、构建、测试。

.DESCRIPTION
    逐段执行，第一个失败的步骤即中止并返回非零退出码，适合直接当提交前的门禁。

    脚本自己补齐两件事，避免"本机能跑、别人机器报错"：
      - 把 cargo 的 bin 目录放进 PATH
      - 清空 RUSTC_WRAPPER / CARGO_BUILD_RUSTC_WRAPPER，
        因为 ~/.cargo/config.toml 配了 sccache 包装，而 sccache 并非每台机器都装

.PARAMETER SkipTests
    只做格式、lint、构建。画面回归需要可用的 Vulkan 适配器，
    在没有 GPU 的环境（CI 容器、纯远程机器）上用它跳过。

.PARAMETER UpdateGolden
    重新生成 tests/golden 下的基线图。改了着色之后才用，并应连同截图一起提交。

.EXAMPLE
    pwsh -File scripts/check.ps1

.EXAMPLE
    pwsh -File scripts/check.ps1 -UpdateGolden
#>
[CmdletBinding()]
param(
    [switch]$SkipTests,
    [switch]$UpdateGolden
)

$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $repo

$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:RUSTC_WRAPPER = ''
$env:CARGO_BUILD_RUSTC_WRAPPER = ''

$steps = @(
    @{ Name = 'fmt'; Run = { cargo fmt --all --check } }
    @{ Name = 'clippy'; Run = { cargo clippy --workspace --all-targets -- -D warnings } }
    @{ Name = 'build'; Run = { cargo build --workspace } }
)
if (-not $SkipTests) {
    $steps += @{ Name = 'test'; Run = { cargo test --workspace } }
}

if ($UpdateGolden) {
    $env:UPDATE_GOLDEN = '1'
    Write-Host 'golden baselines will be regenerated' -ForegroundColor Yellow
}

$failed = @()
foreach ($step in $steps) {
    Write-Host ''
    Write-Host "==> $($step.Name)" -ForegroundColor Cyan
    $started = Get-Date
    & $step.Run
    $code = $LASTEXITCODE
    $seconds = [math]::Round(((Get-Date) - $started).TotalSeconds, 1)
    if ($code -ne 0) {
        Write-Host "<== $($step.Name) failed (exit $code, ${seconds}s)" -ForegroundColor Red
        $failed += $step.Name
        break
    }
    Write-Host "<== $($step.Name) ok (${seconds}s)" -ForegroundColor Green
}

Write-Host ''
if ($failed.Count -gt 0) {
    Write-Host "check failed: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}

Write-Host 'check passed' -ForegroundColor Green
exit 0
