#requires -Version 5.1
<#
.SYNOPSIS
    Build and safely stage the Windows software that lives on the Pocket AI.

.DESCRIPTION
    Builds UnoOne Power, UnoOne Dock, and Start UnoOne. Existing USB files are
    copied into RECOVERY\package-backups before replacement. A complete offline
    voice bundle can be included in the same transaction. Use -SkipBuild with
    binaries produced on a WDAC-allowed build host.
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)]
    [string]$VaultRoot,
    [switch]$SkipBuild,
    [string]$PowerBinaryPath = "",
    [string]$DockBinaryPath = "",
    [string]$StarterBinaryPath = "",
    [string]$VoiceBundleRoot = "",
    [switch]$SkipNativeVerification
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$root = (Resolve-Path -LiteralPath $VaultRoot).Path
$stamp = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmss")
$backupRoot = Join-Path $root "RECOVERY\package-backups\$stamp"

if (-not $SkipBuild) {
    Push-Location (Join-Path $repoRoot "apps\desktop\src")
    try {
        npm ci
        if ($LASTEXITCODE -ne 0) { throw "npm ci failed" }
        npm run build
        if ($LASTEXITCODE -ne 0) { throw "Frontend build failed" }
    } finally {
        Pop-Location
    }

    Push-Location $repoRoot
    try {
        cargo build --release `
            -p unoone-power `
            -p unoone-dock-windows `
            -p unoone-starter-windows
        if ($LASTEXITCODE -ne 0) { throw "Pocket AI Windows application build failed" }
    } finally {
        Pop-Location
    }

    $PowerBinaryPath = Join-Path $repoRoot "target\release\unoone-power.exe"
    $DockBinaryPath = Join-Path $repoRoot "target\release\UnoOneDock.exe"
    $StarterBinaryPath = Join-Path $repoRoot "target\release\start-unoone.exe"
}

$sources = [ordered]@{
    "APPS\WINDOWS\UnoOnePower.exe" = $PowerBinaryPath
    "APPS\WINDOWS\UnoOneDock.exe" = $DockBinaryPath
    "Start UnoOne.exe" = $StarterBinaryPath
}

$voiceSource = $null
$voiceDestination = Join-Path $root "RUNTIMES\WINDOWS\VOICE"
$voiceBackup = Join-Path $backupRoot "RUNTIMES\WINDOWS\VOICE"
$voiceWasPresent = Test-Path -LiteralPath $voiceDestination -PathType Container
if ($VoiceBundleRoot) {
    $bundleRoot = (Resolve-Path -LiteralPath $VoiceBundleRoot).Path
    $voiceSource = Join-Path $bundleRoot "RUNTIMES\WINDOWS\VOICE"
    $voiceModels = Join-Path $bundleRoot "MODELS\DESKTOP"
    foreach ($required in @(
        (Join-Path $voiceSource "whisper.exe"),
        (Join-Path $voiceSource "piper.exe"),
        (Join-Path $voiceSource "espeak-ng-data\phondata"),
        (Join-Path $voiceModels "whisper-base.en.bin"),
        (Join-Path $voiceModels "voice.onnx"),
        (Join-Path $voiceModels "voice.onnx.json")
    )) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "Voice bundle is incomplete: $required"
        }
    }
    $sources["MODELS\DESKTOP\whisper-base.en.bin"] = Join-Path $voiceModels "whisper-base.en.bin"
    $sources["MODELS\DESKTOP\voice.onnx"] = Join-Path $voiceModels "voice.onnx"
    $sources["MODELS\DESKTOP\voice.onnx.json"] = Join-Path $voiceModels "voice.onnx.json"
}

foreach ($entry in $sources.GetEnumerator()) {
    if (-not $entry.Value -or -not (Test-Path -LiteralPath $entry.Value -PathType Leaf)) {
        throw "Built/signed source is missing for $($entry.Key): $($entry.Value)"
    }
}

$originalFiles = @{}

function Backup-And-Copy {
    param([string]$Source, [string]$RelativeDestination)
    $destination = Join-Path $root $RelativeDestination
    $destinationDirectory = Split-Path -Parent $destination
    New-Item -ItemType Directory -Force -Path $destinationDirectory | Out-Null
    $originalFiles[$RelativeDestination] = Test-Path -LiteralPath $destination -PathType Leaf
    if ($originalFiles[$RelativeDestination]) {
        $backup = Join-Path $backupRoot $RelativeDestination
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $backup) | Out-Null
        Copy-Item -LiteralPath $destination -Destination $backup
        Write-Host "Backup: $backup"
        Write-Host "  old SHA-256: $((Get-FileHash -Algorithm SHA256 -LiteralPath $backup).Hash)"
    }
    if ($PSCmdlet.ShouldProcess($destination, "Stage Pocket AI binary")) {
        Copy-Item -LiteralPath $Source -Destination $destination -Force
    }
    Write-Host "Staged: $destination"
    Write-Host "  new SHA-256: $((Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash)"
}

$manifestPath = Join-Path $root "manifest.json"
$manifestBackup = Join-Path $backupRoot "manifest.json"
New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
if (Test-Path -LiteralPath $manifestPath -PathType Leaf) {
    Copy-Item -LiteralPath $manifestPath -Destination $manifestBackup
}
if ($voiceSource -and $voiceWasPresent) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $voiceBackup) | Out-Null
    Copy-Item -LiteralPath $voiceDestination -Destination $voiceBackup -Recurse
}

try {
    if ($voiceSource -and $PSCmdlet.ShouldProcess($voiceDestination, "Stage complete Pocket AI voice runtime")) {
        $resolvedRuntimeRoot = [System.IO.Path]::GetFullPath((Join-Path $root "RUNTIMES\WINDOWS"))
        $resolvedVoiceDestination = [System.IO.Path]::GetFullPath($voiceDestination)
        if (-not $resolvedVoiceDestination.StartsWith(
            $resolvedRuntimeRoot + [System.IO.Path]::DirectorySeparatorChar,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Refusing unsafe voice runtime destination: $resolvedVoiceDestination"
        }
        if (Test-Path -LiteralPath $voiceDestination) {
            Remove-Item -LiteralPath $voiceDestination -Recurse -Force
        }
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $voiceDestination) | Out-Null
        Copy-Item -LiteralPath $voiceSource -Destination $voiceDestination -Recurse
        Write-Host "Staged complete offline voice runtime: $voiceDestination"
    }

    foreach ($entry in $sources.GetEnumerator()) {
        Backup-And-Copy -Source $entry.Value -RelativeDestination $entry.Key
    }

    & (Join-Path $PSScriptRoot "New-UnoOneManifestV2.ps1") -VaultRoot $root -Apply

    & (Join-Path $PSScriptRoot "verify-p1-desktop-usb-assets.ps1") -VaultRoot $root -Strict

    if ($SkipNativeVerification) {
        Write-Warning "Native starter execution was explicitly skipped; manifest and SHA-256 verification still passed."
    } else {
        $nativeVerifier = Start-Process `
            -FilePath (Join-Path $root "Start UnoOne.exe") `
            -ArgumentList @("--verify-only") `
            -Wait `
            -PassThru
        if ($nativeVerifier.ExitCode -ne 0) {
            throw "Native strict Pocket AI verification failed with exit code $($nativeVerifier.ExitCode)"
        }
    }
} catch {
    Write-Warning "Staging failed; restoring the previous Pocket AI package."
    foreach ($entry in $sources.GetEnumerator()) {
        $destination = Join-Path $root $entry.Key
        $backup = Join-Path $backupRoot $entry.Key
        if ($originalFiles[$entry.Key] -and (Test-Path -LiteralPath $backup -PathType Leaf)) {
            Copy-Item -LiteralPath $backup -Destination $destination -Force
        } elseif (Test-Path -LiteralPath $destination -PathType Leaf) {
            Remove-Item -LiteralPath $destination -Force
        }
    }
    if (Test-Path -LiteralPath $manifestBackup -PathType Leaf) {
        Copy-Item -LiteralPath $manifestBackup -Destination $manifestPath -Force
    }
    if ($voiceSource) {
        if (Test-Path -LiteralPath $voiceDestination) {
            Remove-Item -LiteralPath $voiceDestination -Recurse -Force
        }
        if ($voiceWasPresent -and (Test-Path -LiteralPath $voiceBackup -PathType Container)) {
            Copy-Item -LiteralPath $voiceBackup -Destination $voiceDestination -Recurse
        }
    }
    throw
}

if ($SkipNativeVerification) {
    Write-Host "Pocket AI staging and strict manifest/SHA-256 verification passed; native execution was skipped." -ForegroundColor Yellow
} else {
    Write-Host "Pocket AI staging and both strict verification layers passed." -ForegroundColor Green
}
