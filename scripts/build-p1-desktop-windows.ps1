#requires -Version 5.1
<#
.SYNOPSIS
    Build and stage the UnoOne Power Windows desktop binary for P1 acceptance.

.DESCRIPTION
    This script must run on a Windows host where WDAC/AppLocker allows Rust/Cargo
    build scripts and the Tauri CLI to execute. It:
      1. Builds the frontend.
      2. Builds the Tauri release bundle (MSI/NSIS/exe).
      3. Copies the portable exe and installer into D:\UNOONE\APPS\WINDOWS\.
      4. Computes SHA-256 hashes and updates D:\UNOONE\manifest.json.

    On an audit host with strict WDAC, the Tauri release build may be blocked
    (os error 4551). In that case, run this script on a dedicated build host,
    sign the binaries, and then copy the signed output to the USB vault.

.PARAMETER VaultRoot
    Root of the UnoOne USB vault. Default: D:\UNOONE

.PARAMETER SkipBuild
    If set, skip building and only stage/re-hash an already-built binary.

.PARAMETER BinaryPath
    Path to an already-built UnoOne Power.exe to stage. Use with -SkipBuild.

.EXAMPLE
    .\build-p1-desktop-windows.ps1
    .\build-p1-desktop-windows.ps1 -SkipBuild -BinaryPath "C:\build\UnoOne Power.exe"
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$VaultRoot = "D:\UNOONE",
    [switch]$SkipBuild,
    [string]$BinaryPath = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$desktopDir = Join-Path $repoRoot "apps\desktop"
$srcTauriDir = Join-Path $desktopDir "src-tauri"

function Get-Sha256 {
    param([string]$Path)
    $hash = Get-FileHash -Algorithm SHA256 -Path $Path
    return $hash.Hash.ToUpper()
}

# --- Validate vault ---
$manifestPath = Join-Path $VaultRoot "manifest.json"
if (-not (Test-Path $manifestPath)) {
    throw "manifest.json not found at $VaultRoot"
}

$appsDir = Join-Path $VaultRoot "APPS\WINDOWS"
New-Item -ItemType Directory -Force -Path $appsDir | Out-Null

# --- Build ---
if (-not $SkipBuild) {
    Write-Host "Building frontend..." -ForegroundColor Cyan
    Push-Location $desktopDir
    try {
        npm run build | Out-Host
    } finally {
        Pop-Location
    }

    Write-Host "Building Tauri release bundle..." -ForegroundColor Cyan
    Push-Location $desktopDir
    try {
        npm run tauri build | Out-Host
    } finally {
        Pop-Location
    }

    $expectedExe = Join-Path $srcTauriDir "target\release\UnoOne Power.exe"
    if (-not (Test-Path $expectedExe)) {
        throw "Tauri build did not produce '$expectedExe'. Check the build output above."
    }
    $BinaryPath = $expectedExe
}

if (-not $BinaryPath -or -not (Test-Path $BinaryPath)) {
    throw "No built binary found. Run without -SkipBuild or provide -BinaryPath."
}

$exeName = "UnoOnePower.exe"
$destExe = Join-Path $appsDir $exeName

if ($PSCmdlet.ShouldProcess($destExe, "Copy desktop binary")) {
    Copy-Item -Path $BinaryPath -Destination $destExe -Force
}

$exeHash = Get-Sha256 $destExe
$exeSize = (Get-Item $destExe).Length

# Copy installer if it exists
$installerPattern = Join-Path $srcTauriDir "target\release\bundle\*\*.msi"
$installers = Get-ChildItem -Path $installerPattern -File -ErrorAction SilentlyContinue
if ($installers.Count -gt 0) {
    $installer = $installers | Sort-Object Length -Descending | Select-Object -First 1
    $destInstaller = Join-Path $appsDir $installer.Name
    if ($PSCmdlet.ShouldProcess($destInstaller, "Copy installer")) {
        Copy-Item -Path $installer.FullName -Destination $destInstaller -Force
    }
    $installerHash = Get-Sha256 $destInstaller
    $installerSize = (Get-Item $destInstaller).Length
}

# Update manifest
$manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
if (-not $manifest.PSObject.Properties.Match('apps')) {
    $manifest | Add-Member -NotePropertyName 'apps' -NotePropertyValue @{ windows = @{}; macos = @{} } -Force
}
if (-not $manifest.apps.PSObject.Properties.Match('windows')) {
    $manifest.apps | Add-Member -NotePropertyName 'windows' -NotePropertyValue @{} -Force
}

$windowsEntry = @{
    path = "APPS/WINDOWS/"
    entry_point = $exeName
    entry_point_size_bytes = $exeSize
    entry_point_sha256 = $exeHash
    note = "UnoOne Power desktop executable built by build-p1-desktop-windows.ps1"
}
if ($installerHash) {
    $windowsEntry.installer = @{
        name = $installer.Name
        size_bytes = $installerSize
        sha256 = $installerHash
    }
}
$manifest.apps.windows | Add-Member -NotePropertyName 'desktop' -NotePropertyValue $windowsEntry -Force

if ($PSCmdlet.ShouldProcess($manifestPath, "Update manifest.json")) {
    $manifest | ConvertTo-Json -Depth 10 | Set-Content -Path $manifestPath -Encoding UTF8
}

Write-Host ""
Write-Host "Desktop binary staged." -ForegroundColor Green
Write-Host "  Exe:        $destExe"
Write-Host "  SHA-256:    $exeHash"
Write-Host "  Size:       $exeSize"
if ($installerHash) {
    Write-Host "  Installer:  $destInstaller"
}
Write-Host "  Manifest:   $manifestPath"
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Cyan
Write-Host "  1. Sign the binary with the UnoOne code-signing certificate."
Write-Host "  2. Add the signer to the target WDAC/AppLocker policy (or use a signed catalog)."
Write-Host "  3. Run scripts\verify-p1-desktop-usb-assets.ps1 to confirm the vault layout."
