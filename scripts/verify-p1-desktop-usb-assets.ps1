#requires -Version 5.1
<#
.SYNOPSIS
    Verify that the UnoOne USB vault contains all assets required for P1 desktop acceptance.

.DESCRIPTION
    Reads D:\UNOONE\manifest.json, checks that every declared model and runtime file
    exists and that its SHA-256 hash matches the manifest, and reports missing or
    mismatched assets. Also verifies the vault signature files (VERSION, vault.id).

.PARAMETER VaultRoot
    Root of the UnoOne USB vault. Default: D:\UNOONE

.PARAMETER Strict
    If set, treat missing voice/desktop app assets as errors. By default they are
    warnings because they are environmental blockers, not code defects.

.EXAMPLE
    .\verify-p1-desktop-usb-assets.ps1
    .\verify-p1-desktop-usb-assets.ps1 -VaultRoot E:\UNOONE -Strict
#>
[CmdletBinding()]
param(
    [string]$VaultRoot = "D:\UNOONE",
    [switch]$Strict
)

$ErrorActionPreference = "Stop"

function Get-Sha256 {
    param([string]$Path)
    $hash = Get-FileHash -Algorithm SHA256 -Path $Path
    return $hash.Hash.ToUpper()
}

function Test-ManifestEntry {
    param(
        [string]$Label,
        [string]$RelativePath,
        [string]$ExpectedHash,
        [long]$ExpectedSize,
        [string]$VaultRoot
    )

    $fullPath = Join-Path $VaultRoot ($RelativePath -replace '/', '\')
    $result = [PSCustomObject]@{
        Label = $Label
        Path = $RelativePath
        Exists = $false
        HashOk = $null
        SizeOk = $null
        Error = $null
    }

    if (-not (Test-Path $fullPath)) {
        $result.Error = "Missing"
        return $result
    }

    $result.Exists = $true
    $item = Get-Item $fullPath

    if ($ExpectedSize -gt 0) {
        $result.SizeOk = ($item.Length -eq $ExpectedSize)
        if (-not $result.SizeOk) {
            $result.Error = "Size mismatch: expected $ExpectedSize, got $($item.Length)"
        }
    }

    if ($ExpectedHash) {
        $actualHash = Get-Sha256 $fullPath
        $result.HashOk = ($actualHash -eq $ExpectedHash.ToUpper())
        if (-not $result.HashOk) {
            $result.Error = "Hash mismatch: expected $ExpectedHash, got $actualHash"
        }
    }

    return $result
}

function New-ValidationFailure {
    param([string]$Label, [string]$Path, [string]$Error)
    return [PSCustomObject]@{
        Label = $Label
        Path = $Path
        Exists = $true
        HashOk = $false
        SizeOk = $null
        Error = $Error
    }
}

# --- Main ---

$manifestPath = Join-Path $VaultRoot "manifest.json"
if (-not (Test-Path $manifestPath)) {
    throw "manifest.json not found at $VaultRoot -- is this a UnoOne vault?"
}

$versionPath = Join-Path $VaultRoot "VERSION"
if (-not (Test-Path $versionPath)) {
    Write-Warning "VERSION file missing at $VaultRoot"
}

$vaultIdPath = Join-Path $VaultRoot "VAULT\identity\vault.id"
if (-not (Test-Path $vaultIdPath)) {
    Write-Warning "vault.id missing at $VaultRoot"
}

$manifest = Get-Content -Raw -Path $manifestPath | ConvertFrom-Json
$results = @()
$isV2 = $manifest.product_id -eq "com.inbharatai.unoone.pocket-ai" -and $manifest.schema_version -eq 2

if ($isV2) {
    $version = if (Test-Path -LiteralPath $versionPath) {
        (Get-Content -Raw -LiteralPath $versionPath).Trim()
    } else { "" }
    if ($version -ne $manifest.pai_version) {
        $results += [PSCustomObject]@{
            Label = "PAI version"
            Path = "VERSION"
            Exists = (Test-Path -LiteralPath $versionPath)
            HashOk = $null
            SizeOk = $false
            Error = "VERSION '$version' does not match manifest '$($manifest.pai_version)'"
        }
    }

    $assets = @($manifest.platforms.windows.desktop)
    if ($manifest.platforms.windows.dock) { $assets += $manifest.platforms.windows.dock }
    if ($manifest.platforms.windows.starter) { $assets += $manifest.platforms.windows.starter }
    $assets += @($manifest.platforms.windows.runtimes)
    $assets += @($manifest.platforms.windows.models)
    $assets += @($manifest.platforms.windows.voice)

    $windows = $manifest.platforms.windows
    if (-not ($windows.architectures -contains "x86_64")) {
        $results += New-ValidationFailure `
            -Label "Windows architecture" `
            -Path "manifest.json" `
            -Error "The strict Windows package must declare x86_64"
    }

    $kindChecks = @(
        [PSCustomObject]@{ Asset = $windows.desktop; Allowed = @("DESKTOP_EXECUTABLE") },
        [PSCustomObject]@{ Asset = $windows.dock; Allowed = @("DOCK_EXECUTABLE") },
        [PSCustomObject]@{ Asset = $windows.starter; Allowed = @("STARTER_EXECUTABLE") }
    )
    foreach ($asset in @($windows.runtimes)) {
        $kindChecks += [PSCustomObject]@{
            Asset = $asset
            Allowed = @("RUNTIME_EXECUTABLE", "RUNTIME_LIBRARY")
        }
    }
    foreach ($asset in @($windows.models)) {
        $kindChecks += [PSCustomObject]@{
            Asset = $asset
            Allowed = @("MODEL", "MMPROJ")
        }
    }
    foreach ($asset in @($windows.voice)) {
        $kindChecks += [PSCustomObject]@{
            Asset = $asset
            Allowed = @("VOICE_RUNTIME", "RUNTIME_LIBRARY", "WHISPER_MODEL", "PIPER_MODEL")
        }
    }
    foreach ($check in $kindChecks) {
        if (-not $check.Asset) {
            $results += New-ValidationFailure `
                -Label "Required manifest asset" `
                -Path "manifest.json" `
                -Error "Desktop, Dock, and Starter declarations are required"
            continue
        }
        if ($check.Asset.kind -notin $check.Allowed) {
            $results += New-ValidationFailure `
                -Label "Asset kind: $($check.Asset.id)" `
                -Path $check.Asset.path `
                -Error "Kind '$($check.Asset.kind)' is not one of: $($check.Allowed -join ', ')"
        }
    }

    $requiredRuntime = @($windows.runtimes) |
        Where-Object { $_.required -and $_.kind -eq "RUNTIME_EXECUTABLE" } |
        Select-Object -First 1
    $requiredModel = @($windows.models) |
        Where-Object { $_.required -and $_.kind -eq "MODEL" } |
        Select-Object -First 1
    if (-not $requiredRuntime -or -not $requiredModel) {
        $results += New-ValidationFailure `
            -Label "Required launch assets" `
            -Path "manifest.json" `
            -Error "At least one required Windows runtime executable and model must be declared"
    }

    $uniquePaths = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($asset in $assets) {
        $normalizedPath = ([string]$asset.path).Replace('\', '/')
        if (-not $uniquePaths.Add($normalizedPath)) {
            $results += New-ValidationFailure `
                -Label "Duplicate asset path: $($asset.id)" `
                -Path $asset.path `
                -Error "Each declared Windows asset must use a unique path"
        }
        if ($asset.required -and $asset.architecture -and $asset.architecture -ne "x86_64") {
            $results += New-ValidationFailure `
                -Label "Asset architecture: $($asset.id)" `
                -Path $asset.path `
                -Error "Required Windows asset targets '$($asset.architecture)', expected 'x86_64'"
        }
        if (-not $asset.path -or [IO.Path]::IsPathRooted($asset.path) -or $asset.path -match '(^|[\\/])\.\.([\\/]|$)') {
            $results += [PSCustomObject]@{
                Label = "Asset: $($asset.id)"
                Path = $asset.path
                Exists = $false
                HashOk = $false
                SizeOk = $false
                Error = "Unsafe absolute or traversal path"
            }
            continue
        }
        $results += Test-ManifestEntry `
            -Label "Asset: $($asset.id)" `
            -RelativePath $asset.path `
            -ExpectedHash $asset.sha256 `
            -ExpectedSize $asset.size_bytes `
            -VaultRoot $VaultRoot
    }

    $vaultIdRelative = $manifest.vault.id_path
    if (-not $vaultIdRelative -or
        [IO.Path]::IsPathRooted($vaultIdRelative) -or
        $vaultIdRelative -match '(^|[\\/])\.\.([\\/]|$)') {
        $results += New-ValidationFailure `
            -Label "Vault identity path" `
            -Path $vaultIdRelative `
            -Error "vault.id path is empty, absolute, or contains traversal"
        $vaultIdRelative = "VAULT/identity/vault.id"
    }
    $vaultIdFull = Join-Path $VaultRoot ($vaultIdRelative -replace '/', '\')
    $vaultId = if (Test-Path -LiteralPath $vaultIdFull) {
        (Get-Content -Raw -LiteralPath $vaultIdFull).Trim()
    } else { "" }
    if (-not $vaultId -or ($manifest.vault.expected_id -and $vaultId -ne $manifest.vault.expected_id)) {
        $results += [PSCustomObject]@{
            Label = "Vault identity"
            Path = $vaultIdRelative
            Exists = (Test-Path -LiteralPath $vaultIdFull)
            HashOk = $false
            SizeOk = $null
            Error = "vault.id is missing, empty, or does not match the manifest"
        }
    }
    if ($manifest.vault.id_sha256) {
        $results += Test-ManifestEntry `
            -Label "Vault identity hash" `
            -RelativePath $vaultIdRelative `
            -ExpectedHash $manifest.vault.id_sha256 `
            -ExpectedSize 0 `
            -VaultRoot $VaultRoot
    }
} else {
    if ($Strict) {
        $results += [PSCustomObject]@{
            Label = "Strict manifest schema"
            Path = "manifest.json"
            Exists = $false
            HashOk = $false
            SizeOk = $false
            Error = "Legacy manifest rejected in strict mode; schema v2 is required"
        }
    }

# Check models
if ($manifest.models) {
    if ($manifest.models.desktop) {
        $manifest.models.desktop.PSObject.Properties | ForEach-Object {
            $name = $_.Name
            $entry = $_.Value
            $results += Test-ManifestEntry -Label "Model: $name" -RelativePath $entry.path -ExpectedHash $entry.sha256 -ExpectedSize $entry.size_bytes -VaultRoot $VaultRoot
            if ($entry.mmproj_path) {
                $results += Test-ManifestEntry -Label "mmproj: $name" -RelativePath $entry.mmproj_path -ExpectedHash $entry.mmproj_sha256 -ExpectedSize $entry.mmproj_size_bytes -VaultRoot $VaultRoot
            }
            if ($entry.config_path) {
                $results += Test-ManifestEntry -Label "config: $name" -RelativePath $entry.config_path -ExpectedHash $entry.config_sha256 -ExpectedSize $entry.config_size_bytes -VaultRoot $VaultRoot
            }
        }
    }
}

# Check runtimes
if ($manifest.runtimes) {
    if ($manifest.runtimes.windows) {
        $manifest.runtimes.windows.PSObject.Properties | ForEach-Object {
            $name = $_.Name
            $entry = $_.Value
            $dir = Join-Path $VaultRoot ($entry.path -replace '/', '\')

            # Entry point
            if ($entry.entry_point) {
                $results += Test-ManifestEntry -Label "Runtime: $name entry point" -RelativePath "$($entry.path)$($entry.entry_point)" -ExpectedHash $null -ExpectedSize 0 -VaultRoot $VaultRoot
            }
            if ($entry.entry_point_stt) {
                $results += Test-ManifestEntry -Label "Runtime: $name STT entry" -RelativePath "$($entry.path)$($entry.entry_point_stt)" -ExpectedHash $entry.entry_point_stt_sha256 -ExpectedSize $entry.entry_point_stt_size_bytes -VaultRoot $VaultRoot
            }
            if ($entry.entry_point_tts) {
                $results += Test-ManifestEntry -Label "Runtime: $name TTS entry" -RelativePath "$($entry.path)$($entry.entry_point_tts)" -ExpectedHash $entry.entry_point_tts_sha256 -ExpectedSize $entry.entry_point_tts_size_bytes -VaultRoot $VaultRoot
            }

            # Required DLLs (best-effort for entries that declare them)
            if ($entry.required_dlls) {
                foreach ($dll in $entry.required_dlls) {
                    $results += Test-ManifestEntry -Label "Runtime: $name dll $dll" -RelativePath "$($entry.path)$dll" -ExpectedHash $null -ExpectedSize 0 -VaultRoot $VaultRoot
                }
            }
        }
    }
}

# Check desktop app binary (optional / environmental)
$appWindowsPath = Join-Path $VaultRoot "APPS\WINDOWS"
$desktopBinary = Get-ChildItem -File -Path $appWindowsPath -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $desktopBinary) {
    $results += [PSCustomObject]@{
        Label = "Desktop app binary"
        Path = "APPS\WINDOWS\*"
        Exists = $false
        HashOk = $null
        SizeOk = $null
        Error = "No signed desktop binary staged in APPS\WINDOWS\ (environmental blocker)"
    }
}
}

# Summary
$missing = $results | Where-Object { -not $_.Exists }
$bad = $results | Where-Object { $_.Exists -and ($_.HashOk -eq $false -or $_.SizeOk -eq $false) }
$ok = $results | Where-Object { $_.Exists -and ($_.HashOk -ne $false) -and ($_.SizeOk -ne $false) }

Write-Host ""
Write-Host "UnoOne P1 Desktop USB Asset Verification" -ForegroundColor Cyan
Write-Host "  Vault root: $VaultRoot"
Write-Host "  Total checks: $($results.Count)"
Write-Host "  OK: $($ok.Count)" -ForegroundColor Green
if ($bad.Count -gt 0) {
    Write-Host "  Hash/Size mismatches: $($bad.Count)" -ForegroundColor Red
}
if ($missing.Count -gt 0) {
    Write-Host "  Missing: $($missing.Count)" -ForegroundColor $(if ($Strict) { 'Red' } else { 'Yellow' })
}

if ($bad.Count -gt 0) {
    Write-Host ""
    Write-Host "Mismatches:" -ForegroundColor Red
    $bad | ForEach-Object { Write-Host "  [FAIL] $($_.Label) : $($_.Error)" -ForegroundColor Red }
}

if ($missing.Count -gt 0) {
    Write-Host ""
    Write-Host "Missing assets:" -ForegroundColor $(if ($Strict) { 'Red' } else { 'Yellow' })
    $missing | ForEach-Object { Write-Host "  [MISS] $($_.Label) : $($_.Path)" -ForegroundColor $(if ($Strict) { 'Red' } else { 'Yellow' }) }
}

if ($bad.Count -eq 0 -and ($missing.Count -eq 0 -or -not $Strict)) {
    Write-Host ""
    Write-Host "Verification passed." -ForegroundColor Green
}

if ($Strict -and ($bad.Count -gt 0 -or $missing.Count -gt 0)) {
    throw "Strict Pocket AI verification failed: $($bad.Count) mismatch(es), $($missing.Count) missing asset(s)"
}
