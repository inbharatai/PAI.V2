#requires -Version 5.1
<#
.SYNOPSIS
    Run the full P1 desktop local gate suite.

.DESCRIPTION
    Executes, in order:
      1. cargo fmt --all --check
      2. cargo check
      3. cargo clippy -- -D warnings
      4. cargo test --workspace
      5. cargo build -p unoone-power
      6. npm run lint
      7. npm run build

    Any failure stops the script and returns the failing exit code.
    This script does NOT run `npm run tauri build` because the Tauri release
    bundle requires a WDAC-allowed build host on many Windows audit machines.

.PARAMETER SkipRust
    Skip the Rust gates (useful if only frontend changed).

.PARAMETER SkipFrontend
    Skip the frontend gates (useful if only Rust changed).

.EXAMPLE
    .\run-p1-desktop-gates.ps1
    .\run-p1-desktop-gates.ps1 -SkipFrontend
#>
[CmdletBinding()]
param(
    [switch]$SkipRust,
    [switch]$SkipFrontend
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$desktopDir = Join-Path $repoRoot "apps\desktop"

function Invoke-Step {
    param(
        [string]$Name,
        [scriptblock]$Action
    )
    Write-Host ""
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    & $Action
    if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne $null) {
        Write-Host "FAILED: $Name (exit $LASTEXITCODE)" -ForegroundColor Red
        exit $LASTEXITCODE
    }
    Write-Host "OK: $Name" -ForegroundColor Green
}

Push-Location $repoRoot
try {
    if (-not $SkipRust) {
        Invoke-Step "Rust format check" { cargo fmt --all --check }
        Invoke-Step "Rust check" { cargo check }
        Invoke-Step "Rust clippy" { cargo clippy -- -D warnings }
        Invoke-Step "Workspace tests" { cargo test --workspace }
        Invoke-Step "Rust debug binary link" { cargo build -p unoone-power }
    }

    if (-not $SkipFrontend) {
        Push-Location $desktopDir
        try {
            Invoke-Step "Frontend lint" { npm run lint }
            Invoke-Step "Frontend build" { npm run build }
        } finally {
            Pop-Location
        }
    }
} finally {
    Pop-Location
}

Write-Host ""
Write-Host "All P1 desktop gates passed." -ForegroundColor Green
