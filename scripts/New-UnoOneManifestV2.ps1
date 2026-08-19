#requires -Version 5.1
<#
.SYNOPSIS
    Generate the strict schema-v2 manifest for a physical Pocket AI pen drive.

.DESCRIPTION
    Hashes the actual staged Windows applications, every Windows runtime file,
    every desktop model, and any declared voice assets. By default it writes a
    candidate beside the repository. Use -Apply to back up and replace the
    physical manifest only after all binaries have been built and, for a
    production release, Authenticode-signed.
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)]
    [string]$VaultRoot,
    [string]$OutputPath = "",
    [switch]$Apply
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path -LiteralPath $VaultRoot).Path
$versionPath = Join-Path $root "VERSION"
$vaultIdPath = Join-Path $root "VAULT\identity\vault.id"
$desktopPath = Join-Path $root "APPS\WINDOWS\UnoOnePower.exe"
$dockPath = Join-Path $root "APPS\WINDOWS\UnoOneDock.exe"
$starterPath = Join-Path $root "Start UnoOne.exe"

foreach ($required in @($versionPath, $vaultIdPath, $desktopPath, $dockPath, $starterPath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required staged Pocket AI file is missing: $required"
    }
}

function Get-RelativePath {
    param([string]$Path)
    $relative = $Path.Substring($root.Length).TrimStart('\', '/')
    return $relative.Replace('\', '/')
}

function New-Asset {
    param(
        [System.IO.FileInfo]$File,
        [string]$Kind,
        [string]$Id,
        [string]$Architecture = "x86_64"
    )
    return [ordered]@{
        id = $Id
        kind = $Kind
        path = Get-RelativePath -Path $File.FullName
        size_bytes = $File.Length
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $File.FullName).Hash.ToUpperInvariant()
        required = $true
        architecture = $Architecture
    }
}

function Get-AssetKind {
    param([System.IO.FileInfo]$File, [string]$Area)
    if ($Area -eq "model") {
        if ($File.Name -match '(?i)mmproj') { return "MMPROJ" }
        if ($File.Name -match '(?i)whisper') { return "WHISPER_MODEL" }
        if ($File.Name -match '(?i)piper|voice') { return "PIPER_MODEL" }
        return "MODEL"
    }
    if ($File.Extension -ieq ".exe") { return "RUNTIME_EXECUTABLE" }
    return "RUNTIME_LIBRARY"
}

$runtimeRoot = Join-Path $root "RUNTIMES\WINDOWS"
$modelRoot = Join-Path $root "MODELS\DESKTOP"
$mobileModelRoot = Join-Path $root "MODELS\MOBILE"
if (-not (Test-Path -LiteralPath $runtimeRoot -PathType Container)) {
    throw "Windows runtimes are missing: $runtimeRoot"
}
if (-not (Test-Path -LiteralPath $modelRoot -PathType Container)) {
    throw "Desktop models are missing: $modelRoot"
}

$runtimeAssets = @()
$voiceAssets = @()
foreach ($file in Get-ChildItem -LiteralPath $runtimeRoot -Recurse -File | Sort-Object FullName) {
    $kind = Get-AssetKind -File $file -Area "runtime"
    $asset = New-Asset -File $file -Kind $kind -Id ("runtime-" + (Get-RelativePath $file.FullName).ToLowerInvariant())
    if ($file.FullName -match '(?i)[\\/]VOICE[\\/]|whisper|piper') {
        if ($file.Extension -ieq ".exe") { $asset.kind = "VOICE_RUNTIME" }
        $voiceAssets += $asset
    } else {
        $runtimeAssets += $asset
    }
}

$modelAssets = @()
foreach ($file in Get-ChildItem -LiteralPath $modelRoot -Recurse -File | Sort-Object FullName) {
    $kind = Get-AssetKind -File $file -Area "model"
    $asset = New-Asset -File $file -Kind $kind -Id ("model-" + $file.BaseName.ToLowerInvariant())
    if ($kind -in @("WHISPER_MODEL", "PIPER_MODEL")) {
        $voiceAssets += $asset
    } else {
        $modelAssets += $asset
    }
}

$mobileModelAssets = @()
if (Test-Path -LiteralPath $mobileModelRoot -PathType Container) {
    foreach ($file in Get-ChildItem -LiteralPath $mobileModelRoot -Recurse -File | Sort-Object FullName) {
        $mobileModelAssets += New-Asset `
            -File $file `
            -Kind "MOBILE_MODEL" `
            -Id ("mobile-model-" + $file.BaseName.ToLowerInvariant()) `
            -Architecture "arm64-v8a"
    }
}

$vaultId = (Get-Content -Raw -LiteralPath $vaultIdPath).Trim()
if (-not $vaultId) { throw "vault.id is empty: $vaultIdPath" }
$version = (Get-Content -Raw -LiteralPath $versionPath).Trim()
if (-not $version) { throw "VERSION is empty: $versionPath" }

$manifest = [ordered]@{
    product_id = "com.inbharatai.unoone.pocket-ai"
    schema_version = 2
    pai_version = $version
    created_utc = [DateTime]::UtcNow.ToString("o")
    vault = [ordered]@{
        id_path = "VAULT/identity/vault.id"
        expected_id = $vaultId
        id_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $vaultIdPath).Hash.ToUpperInvariant()
    }
    platforms = [ordered]@{
        windows = [ordered]@{
            architectures = @("x86_64")
            desktop = New-Asset -File (Get-Item -LiteralPath $desktopPath) -Kind "DESKTOP_EXECUTABLE" -Id "unoone-power"
            dock = New-Asset -File (Get-Item -LiteralPath $dockPath) -Kind "DOCK_EXECUTABLE" -Id "unoone-dock"
            starter = New-Asset -File (Get-Item -LiteralPath $starterPath) -Kind "STARTER_EXECUTABLE" -Id "start-unoone"
            runtimes = @($runtimeAssets)
            models = @($modelAssets)
            voice = @($voiceAssets)
        }
        mobile = [ordered]@{
            architectures = @("arm64-v8a")
            models = @($mobileModelAssets)
        }
    }
}

$json = $manifest | ConvertTo-Json -Depth 12
# The native Rust verifier (serde_json) rejects a UTF-8 byte-order mark, while
# ConvertFrom-Json silently tolerates one. Windows PowerShell 5.1 Set-Content
# -Encoding UTF8 emits a BOM, so always write the manifest without one.
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
if ($Apply) {
    $destination = Join-Path $root "manifest.json"
    $stamp = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
    $backupRoot = Join-Path $root "RECOVERY\package-backups\$stamp"
    New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
    if (Test-Path -LiteralPath $destination) {
        $backup = Join-Path $backupRoot "manifest.json"
        Copy-Item -LiteralPath $destination -Destination $backup
        $oldHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $backup).Hash
        Write-Host "Backed up manifest ($oldHash) to $backup"
    }
    if ($PSCmdlet.ShouldProcess($destination, "Replace with strict schema-v2 manifest")) {
        [System.IO.File]::WriteAllText($destination, $json, $utf8NoBom)
    }
    $OutputPath = $destination
} else {
    if (-not $OutputPath) {
        $OutputPath = Join-Path (Split-Path -Parent $PSScriptRoot) "manifest.v2.candidate.json"
    }
    [System.IO.File]::WriteAllText($OutputPath, $json, $utf8NoBom)
}

Write-Host "Manifest written: $OutputPath"
Write-Host "Runtime assets: $($runtimeAssets.Count)"
Write-Host "Model assets:   $($modelAssets.Count)"
Write-Host "Voice assets:   $($voiceAssets.Count)"
Write-Host "Mobile models:  $($mobileModelAssets.Count)"
