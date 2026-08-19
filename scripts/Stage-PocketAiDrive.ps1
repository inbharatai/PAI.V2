#requires -Version 5.1
<#
.SYNOPSIS
    Stage the Windows apps from a green-CI Pocket AI bundle onto a physical
    Pocket AI drive (or the desktop COPY) — safely, transactionally, repeatably.

.DESCRIPTION
    The drive is an OUTPUT, never a git input: the repo holds source, the drive
    holds built binaries + multi-GB models + the vault. The ONLY thing that
    changes on the drive when desktop code lands is the three application
    executables. This script replaces exactly those three from a downloaded
    "Pocket AI Windows Bundle" CI artifact and leaves everything else — models,
    runtimes, VAULT, CONFIG, VERSION — untouched.

    It refuses to guess. Every step gates the next:
      1. Verify the bundle against its own SHA256SUMS.txt.
      2. Back up the three existing drive exes to RECOVERY\package-backups\<stamp>.
      3. Copy the three exes into the drive's real layout
         (APPS\WINDOWS\UnoOnePower.exe, APPS\WINDOWS\UnoOneDock.exe,
         "Start UnoOne.exe" at the root).
      4. Run the frontend-embedding gate against the staged UnoOnePower.exe
         (this gate exists because a drive once shipped a UI-less binary).
      5. Regenerate the strict schema-v2 manifest over the ACTUAL drive assets
         (New-UnoOneManifestV2.ps1 -Apply — it backs up the old manifest first).
      6. Run "Start UnoOne.exe --verify-only" as the final drive-side check.

    On any failure it stops and tells you how to roll back from the backup.
    Nothing is deleted; the only copy of a model/runtime/record is never touched.

.PARAMETER VaultRoot
    The drive root, e.g. E:\ , or the copy $env:USERPROFILE\Desktop\UNOONE.

.PARAMETER BundleDir
    The extracted "pocket-ai-windows-x86_64-<sha>" artifact folder containing
    UnoOnePower.exe, UnoOneDock.exe, "Start UnoOne.exe", SHA256SUMS.txt.

.PARAMETER RepoRoot
    Path to a checkout of inbharatai/PAI at the SAME commit as the bundle
    (provides the embedding gate, the manifest tool, and apps/desktop/src/dist
    for the gate). Defaults to the current directory.

.PARAMETER SkipEmbeddingGate
    Only if apps/desktop/src/dist is unavailable in the checkout. Not advised.

.EXAMPLE
    # From a repo checkout at the bundle's commit:
    powershell -ExecutionPolicy Bypass -File scripts\Stage-PocketAiDrive.ps1 `
        -VaultRoot "$env:USERPROFILE\Desktop\UNOONE" `
        -BundleDir "$env:USERPROFILE\Downloads\pocket-ai-windows-x86_64-ce9aaa8"
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [Parameter(Mandatory = $true)] [string]$VaultRoot,
    [Parameter(Mandatory = $true)] [string]$BundleDir,
    [string]$RepoRoot = (Get-Location).Path,
    [switch]$SkipEmbeddingGate
)

$ErrorActionPreference = "Stop"

function Fail($msg) { Write-Host "`n  FAIL: $msg`n" -ForegroundColor Red; exit 1 }
function Step($msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }

$root   = (Resolve-Path -LiteralPath $VaultRoot).Path
$bundle = (Resolve-Path -LiteralPath $BundleDir).Path
$repo   = (Resolve-Path -LiteralPath $RepoRoot).Path
$stamp  = Get-Date -Format "yyyyMMdd-HHmmss"

# The three bundle files and where each lands on the drive.
$plan = @(
    @{ Bundle = "UnoOnePower.exe";  Drive = "APPS\WINDOWS\UnoOnePower.exe" },
    @{ Bundle = "UnoOneDock.exe";   Drive = "APPS\WINDOWS\UnoOneDock.exe"  },
    @{ Bundle = "Start UnoOne.exe"; Drive = "Start UnoOne.exe"             }
)

Step "0. Sanity — drive looks like a Pocket AI"
foreach ($required in @("VERSION", "manifest.json", "VAULT\identity\vault.id")) {
    if (-not (Test-Path -LiteralPath (Join-Path $root $required))) {
        Fail "Not a Pocket AI drive root (missing $required): $root"
    }
}
Write-Host "Drive: $root"
Write-Host "VERSION: $(Get-Content -LiteralPath (Join-Path $root 'VERSION') -Raw)".Trim()

Step "1. Verify the bundle against its own SHA256SUMS.txt"
$sumsPath = Join-Path $bundle "SHA256SUMS.txt"
if (-not (Test-Path -LiteralPath $sumsPath)) { Fail "Bundle is missing SHA256SUMS.txt: $bundle" }
foreach ($line in Get-Content -LiteralPath $sumsPath) {
    if ($line -notmatch '^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$') { continue }
    $want = $Matches[1].ToUpperInvariant(); $name = $Matches[2]
    $f = Join-Path $bundle $name
    if (-not (Test-Path -LiteralPath $f)) { Fail "Bundle lists $name but the file is missing" }
    $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $f).Hash.ToUpperInvariant()
    if ($got -ne $want) { Fail "Bundle hash mismatch for ${name}: want $want got $got" }
    Write-Host "  OK  $name  $got"
}
foreach ($item in $plan) {
    if (-not (Test-Path -LiteralPath (Join-Path $bundle $item.Bundle))) {
        Fail "Bundle is missing a required app: $($item.Bundle)"
    }
}

Step "2. Back up the drive's current exes → RECOVERY\package-backups\$stamp"
$backupRoot = Join-Path $root "RECOVERY\package-backups\$stamp"
New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
foreach ($item in $plan) {
    $dst = Join-Path $root $item.Drive
    if (Test-Path -LiteralPath $dst) {
        $flat = ($item.Drive -replace '[\\/]', '__')
        $bak  = Join-Path $backupRoot $flat
        Copy-Item -LiteralPath $dst -Destination $bak
        $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $bak).Hash.Substring(0,8)
        Write-Host "  backed up $($item.Drive)  ($h)  → $bak"
    } else {
        Write-Host "  (new)     $($item.Drive)  — no prior file to back up"
    }
}
Write-Host "Rollback if needed: copy the files in $backupRoot back to their drive paths, then re-run step 5."

Step "3. Stage the three exes into the drive layout"
foreach ($item in $plan) {
    $src = Join-Path $bundle $item.Bundle
    $dst = Join-Path $root  $item.Drive
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
    if ($PSCmdlet.ShouldProcess($dst, "Copy $($item.Bundle)")) {
        Copy-Item -LiteralPath $src -Destination $dst -Force
        $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $dst).Hash.Substring(0,8)
        Write-Host "  staged   $($item.Drive)  ($h)"
    }
}

Step "4. Frontend-embedding gate on the staged UnoOnePower.exe"
if ($SkipEmbeddingGate) {
    Write-Host "  SKIPPED by request — not advised."
} else {
    $dist = Join-Path $repo "apps\desktop\src\dist"
    $exe  = Join-Path $root "APPS\WINDOWS\UnoOnePower.exe"
    if (-not (Test-Path -LiteralPath $dist)) {
        Fail "Embedding gate needs apps/desktop/src/dist in the repo checkout ($dist). Build the frontend at the bundle's commit, or pass -SkipEmbeddingGate (not advised)."
    }
    & node (Join-Path $repo "scripts\verify-frontend-embedded.mjs") --dist $dist --exe $exe --json
    if ($LASTEXITCODE -ne 0) { Fail "Embedding gate FAILED on the staged exe — the drive was NOT finalized. Roll back from $backupRoot." }
    Write-Host "  embedding gate PASS"
}

Step "5. Regenerate the strict schema-v2 manifest over the drive's real assets"
& (Join-Path $repo "scripts\New-UnoOneManifestV2.ps1") -VaultRoot $root -Apply
if ($LASTEXITCODE -ne 0) { Fail "Manifest regeneration FAILED. Roll back exes + manifest from $backupRoot." }

Step "6. Final drive-side check: Start UnoOne.exe --verify-only"
$starter = Join-Path $root "Start UnoOne.exe"
& $starter --verify-only
if ($LASTEXITCODE -ne 0) { Fail "--verify-only reported a problem (exit $LASTEXITCODE). Roll back from $backupRoot." }

Write-Host "`n  DONE — drive staged to the bundle in $bundle." -ForegroundColor Green
Write-Host "  Old exes preserved in $backupRoot. Launch the app: the UI should open and the model answer." -ForegroundColor Green
