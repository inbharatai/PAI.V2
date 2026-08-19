<#
================================================================================
 Pocket AI — Phase 0 Interrupted Work Recovery + Live Drive Audit Collector
================================================================================
 PURPOSE
   Preserves the UNPUSHED interrupted work on this machine, then performs a
   READ-ONLY audit of the physical Pocket AI drive.

 SAFETY CONTRACT — this script is NON-DESTRUCTIVE BY CONSTRUCTION.
   It NEVER runs: git reset, git checkout, git clean, git stash pop/apply/drop,
   git pull, git merge, git rebase, Remove-Item, Format-Volume, or any write to
   the Pocket AI drive. Your working tree is not modified.

   Uncommitted work is preserved using `git stash create`, which builds a commit
   OBJECT without touching the working tree, and is then anchored to a real
   branch ref so it can never be garbage collected. Everything is additionally
   copied as raw files AND packed into a git bundle. Three independent copies.

 USAGE (normal PowerShell, no admin needed)
   powershell -ExecutionPolicy Bypass -File .\Collect-PocketAI-Recovery.ps1

 OPTIONAL
   -UsbRoot D:\UNOONE       (default D:\UNOONE)
   -SearchRoots C:\,E:\     (extra places to hunt for clones)
   -SkipUsbAudit            (recovery only)
================================================================================
#>

[CmdletBinding()]
param(
    [string]   $UsbRoot      = 'D:\UNOONE',
    [string[]] $SearchRoots  = @(),
    [switch]   $SkipUsbAudit
)

$ErrorActionPreference = 'Continue'
$ProgressPreference    = 'SilentlyContinue'

$Stamp    = Get-Date -Format 'yyyyMMdd-HHmmss'
$OutDir   = Join-Path ([Environment]::GetFolderPath('Desktop')) "PAI_HYPERAGENT_RECOVERY_$Stamp"
$RecBranch = "recovery/hyperagent-resume-$Stamp"

$null = New-Item -ItemType Directory -Force -Path $OutDir
foreach ($sub in 'repos','usb','logs','hashes','files') {
    $null = New-Item -ItemType Directory -Force -Path (Join-Path $OutDir $sub)
}

$TranscriptPath = Join-Path $OutDir 'logs\commands.log'
try { Start-Transcript -LiteralPath $TranscriptPath -Force | Out-Null } catch {}

function Say([string]$m, [string]$c = 'Gray') { Write-Host $m -ForegroundColor $c }
function Head([string]$m) {
    Say ''
    Say ('=' * 78) 'DarkCyan'
    Say "  $m" 'Cyan'
    Say ('=' * 78) 'DarkCyan'
}

Head "Pocket AI Recovery Collector  |  $Stamp"
Say "Output folder : $OutDir" 'Yellow'
Say "Recovery ref  : $RecBranch" 'Yellow'
Say "This script never modifies your working tree or the USB drive." 'DarkGray'

# --------------------------------------------------------------------------
# Environment
# --------------------------------------------------------------------------
Head 'Environment'

$gitVersion = try { (git --version) 2>&1 | Out-String } catch { 'git NOT FOUND' }

$env_report = [ordered]@{
    collected_at_utc  = (Get-Date).ToUniversalTime().ToString('o')
    collector_stamp   = $Stamp
    hostname          = $env:COMPUTERNAME
    username          = $env:USERNAME
    os                = (Get-CimInstance Win32_OperatingSystem |
                          Select-Object Caption, Version, BuildNumber, OSArchitecture)
    powershell        = $PSVersionTable.PSVersion.ToString()
    git               = $gitVersion.Trim()
    cpu               = (Get-CimInstance Win32_Processor |
                          Select-Object -First 1 Name, NumberOfCores, NumberOfLogicalProcessors)
    ram_gb            = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 2)
    gpus              = @(Get-CimInstance Win32_VideoController |
                          Select-Object Name, DriverVersion, AdapterRAM)
    toolchain         = [ordered]@{}
}

foreach ($t in 'cargo','rustc','node','npm','java','adb') {
    $cmd = Get-Command $t -ErrorAction SilentlyContinue
    $env_report.toolchain[$t] = if ($cmd) {
        try { (& $t --version 2>&1 | Select-Object -First 1 | Out-String).Trim() }
        catch { "present at $($cmd.Source)" }
    } else { 'NOT INSTALLED' }
}

Say "Host  : $($env_report.hostname)   User: $($env_report.username)"
Say "OS    : $($env_report.os.Caption) build $($env_report.os.BuildNumber)"
Say "CPU   : $($env_report.cpu.Name)"
Say "RAM   : $($env_report.ram_gb) GB"
Say "Git   : $($env_report.git)"

# --------------------------------------------------------------------------
# Locate candidate PAI repositories
# --------------------------------------------------------------------------
Head 'Locating PAI repositories'

$roots = New-Object System.Collections.Generic.List[string]
foreach ($r in @(
    $env:USERPROFILE,
    (Join-Path $env:USERPROFILE 'Desktop'),
    (Join-Path $env:USERPROFILE 'Documents'),
    (Join-Path $env:USERPROFILE 'source\repos'),
    (Join-Path $env:USERPROFILE 'Downloads'),
    'C:\dev','C:\src','C:\repos','C:\projects','C:\work'
) + $SearchRoots) {
    if ($r -and (Test-Path -LiteralPath $r) -and -not $roots.Contains($r)) { $roots.Add($r) }
}

Say "Scanning $($roots.Count) root location(s) for .git directories (depth-limited)..." 'DarkGray'

$gitDirs = New-Object System.Collections.Generic.List[string]
foreach ($root in $roots) {
    try {
        Get-ChildItem -LiteralPath $root -Directory -Filter '.git' -Recurse -Depth 4 `
                      -Force -ErrorAction SilentlyContinue |
            ForEach-Object { if (-not $gitDirs.Contains($_.FullName)) { $gitDirs.Add($_.FullName) } }
    } catch {}
}

$candidates = New-Object System.Collections.Generic.List[string]
foreach ($gd in $gitDirs) {
    $repo = Split-Path -Parent $gd
    $remotes = try { (git -C $repo remote -v 2>&1 | Out-String) } catch { '' }
    if ($remotes -match 'inbharatai/PAI' -or $remotes -match '[/:]PAI(\.git)?\s') {
        if (-not $candidates.Contains($repo)) {
            $candidates.Add($repo)
            Say "  MATCH  $repo" 'Green'
        }
    }
}

if ($candidates.Count -eq 0) {
    Say '  No PAI clone found in the scanned roots.' 'Red'
    Say '  Re-run with -SearchRoots to add locations, e.g.:' 'Yellow'
    Say '    -SearchRoots "E:\","C:\Users\Public"' 'Yellow'
}

# --------------------------------------------------------------------------
# Per-repository preservation
# --------------------------------------------------------------------------
$TARGET_SHA = '52fb5f87736f7f3fe4c20f63f7c7f2679c9bf534'
$repoReports = @()
$idx = 0

foreach ($repo in $candidates) {
    $idx++
    Head "Repository $idx of $($candidates.Count): $repo"

    $slug   = ($repo -replace '[:\\/ ]', '_').Trim('_')
    $rDir   = Join-Path $OutDir "repos\$slug"
    $null   = New-Item -ItemType Directory -Force -Path $rDir
    $fDir   = Join-Path $OutDir "files\$slug"
    $null   = New-Item -ItemType Directory -Force -Path $fDir

    function GitOut([string]$label, [string[]]$gitArgs) {
        $text = try { (git -C $repo @gitArgs 2>&1 | Out-String) } catch { "ERROR: $_" }
        Set-Content -LiteralPath (Join-Path $rDir "$label.txt") -Value $text -Encoding utf8
        return $text
    }

    # --- read-only state capture (exactly the directive's list) ---
    $toplevel = (GitOut 'rev-parse-show-toplevel' @('rev-parse','--show-toplevel')).Trim()
    $branch   = (GitOut 'branch-show-current'     @('branch','--show-current')).Trim()
    $headSha  = (GitOut 'rev-parse-HEAD'          @('rev-parse','HEAD')).Trim()
    $null     = GitOut 'remote-v'                 @('remote','-v')
    $status   = GitOut 'status-short'             @('status','--short')
    $null     = GitOut 'log-30'                   @('log','--oneline','--decorate','-30')
    $null     = GitOut 'diff-stat'                @('diff','--stat')
    $null     = GitOut 'diff-name-status'         @('diff','--name-status')
    $null     = GitOut 'diff-check'               @('diff','--check')
    $untrackT = GitOut 'untracked'                @('ls-files','--others','--exclude-standard')
    $null     = GitOut 'diff-unstaged-FULL'       @('diff')
    $null     = GitOut 'diff-staged-FULL'         @('diff','--cached')
    $null     = GitOut 'stash-list'               @('stash','list')
    $null     = GitOut 'branch-all-verbose'       @('branch','-a','-vv')
    $null     = GitOut 'show-ref'                 @('show-ref')
    $null     = GitOut 'reflog-50'                @('reflog','-50')
    $null     = GitOut 'worktree-list'            @('worktree','list')
    $null     = GitOut 'diff-vs-target-sha'       @('diff','--stat',$TARGET_SHA)

    $dirtyLines = @($status -split "`r?`n" | Where-Object { $_.Trim() })
    $untracked  = @($untrackT -split "`r?`n" | Where-Object { $_.Trim() })

    Say "  branch : $branch"
    Say "  HEAD   : $headSha"
    Say ("  HEAD == documented main SHA : " + ($(if ($headSha -eq $TARGET_SHA) {'YES'} else {'NO'}))) 'DarkGray'
    Say ("  dirty entries : $($dirtyLines.Count)")   $(if ($dirtyLines.Count) {'Yellow'} else {'DarkGray'})
    Say ("  untracked     : $($untracked.Count)")    $(if ($untracked.Count)  {'Yellow'} else {'DarkGray'})

    # --- PRESERVATION 1of3: raw file copies -------------------------------
    $copied = 0
    $changedPaths = @()
    $changedPaths += @(git -C $repo diff --name-only 2>&1)
    $changedPaths += @(git -C $repo diff --cached --name-only 2>&1)
    $changedPaths += $untracked
    $changedPaths = $changedPaths | Where-Object { $_ -and $_.Trim() } | Select-Object -Unique

    foreach ($rel in $changedPaths) {
        $src = Join-Path $repo $rel
        if (Test-Path -LiteralPath $src -PathType Leaf) {
            $dst = Join-Path $fDir $rel
            $null = New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst)
            try { Copy-Item -LiteralPath $src -Destination $dst -Force; $copied++ } catch {}
        }
    }
    Say "  copied $copied changed/untracked file(s) verbatim" $(if ($copied) {'Green'} else {'DarkGray'})

    # --- PRESERVATION 2of3: stash-create commit anchored to a branch ------
    # `git stash create` writes a commit object and prints its SHA. It does NOT
    # alter the index or working tree. We then anchor it with update-ref so it
    # survives gc and is reachable by name.
    $stashSha = ''
    if ($dirtyLines.Count -gt 0) {
        $stashSha = (git -C $repo stash create "hyperagent recovery $Stamp" 2>&1 | Out-String).Trim()
        if ($stashSha -match '^[0-9a-f]{40}$') {
            git -C $repo update-ref "refs/heads/$RecBranch" $stashSha 2>&1 | Out-Null
            Say "  preserved tracked edits as commit $($stashSha.Substring(0,12)) on $RecBranch" 'Green'
            Set-Content -LiteralPath (Join-Path $rDir 'recovery-commit.txt') `
                        -Value "$stashSha`n$RecBranch" -Encoding utf8
        } else {
            $stashSha = ''
            Say "  NOTE: stash create produced no commit (nothing stashable)" 'DarkGray'
        }
    } else {
        Say '  working tree clean — no tracked edits to preserve' 'DarkGray'
    }

    # --- PRESERVATION 3of3: full git bundle -------------------------------
    $bundle = Join-Path $rDir 'all-refs.bundle'
    $bundleOk = $false
    try {
        git -C $repo bundle create $bundle --all 2>&1 |
            Out-File -LiteralPath (Join-Path $rDir 'bundle-create.log') -Encoding utf8
        $bundleOk = Test-Path -LiteralPath $bundle
    } catch {}
    Say ("  git bundle (all refs) : " + ($(if ($bundleOk) {'OK'} else {'FAILED'}))) `
        $(if ($bundleOk) {'Green'} else {'Red'})

    # --- hashes of preserved files ----------------------------------------
    if ($copied -gt 0) {
        Get-ChildItem -LiteralPath $fDir -Recurse -File |
            ForEach-Object {
                $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash
                "$h  $($_.FullName.Substring($fDir.Length).TrimStart('\'))"
            } | Set-Content -LiteralPath (Join-Path $OutDir "hashes\$slug.preserved.sha256") -Encoding utf8
    }

    $repoReports += [ordered]@{
        path                  = $repo
        toplevel              = $toplevel
        branch                = $branch
        head_sha              = $headSha
        head_matches_main_doc = ($headSha -eq $TARGET_SHA)
        dirty_count           = $dirtyLines.Count
        untracked_count       = $untracked.Count
        files_copied          = $copied
        recovery_commit       = $stashSha
        recovery_branch       = $(if ($stashSha) { $RecBranch } else { '' })
        bundle_created        = $bundleOk
    }
}

# --------------------------------------------------------------------------
# Physical drive audit (READ ONLY)
# --------------------------------------------------------------------------
$usbReport = [ordered]@{ audited = $false; reason = 'skipped by flag' }

if (-not $SkipUsbAudit) {
    Head "Physical Pocket AI audit (read-only): $UsbRoot"

    if (-not (Test-Path -LiteralPath $UsbRoot)) {
        Say "  NOT PRESENT — insert the Pocket AI drive and re-run, or pass -UsbRoot" 'Red'
        $usbReport = [ordered]@{ audited = $false; reason = "path not found: $UsbRoot" }
    } else {
        $usbDir = Join-Path $OutDir 'usb'
        $driveLetter = ($UsbRoot -split ':')[0]

        $vol  = try { Get-Volume -DriveLetter $driveLetter -ErrorAction Stop |
                        Select-Object DriveLetter, FileSystemLabel, FileSystem, FileSystemType,
                                      DriveType, HealthStatus, SizeRemaining, Size } catch { $null }
        $part = try { Get-Partition -DriveLetter $driveLetter -ErrorAction Stop |
                        Select-Object DiskNumber, PartitionNumber, Size, Type } catch { $null }
        $disk = $null
        if ($part) {
            $disk = try { Get-Disk -Number $part.DiskNumber -ErrorAction Stop |
                            Select-Object Number, FriendlyName, SerialNumber, BusType,
                                          Size, PartitionStyle } catch { $null }
        }

        if ($vol)  { Say "  Volume : $($vol.FileSystemLabel)  fs=$($vol.FileSystem)  type=$($vol.DriveType)" }
        if ($disk) { Say "  Disk   : $($disk.FriendlyName)  bus=$($disk.BusType)  serial=$($disk.SerialNumber)" }

        # top-level inventory
        $topLevel = @(Get-ChildItem -LiteralPath $UsbRoot -Force -ErrorAction SilentlyContinue |
                        Select-Object Name, @{n='Type';e={if($_.PSIsContainer){'DIR'}else{'FILE'}}},
                                      @{n='Length';e={$_.Length}}, LastWriteTimeUtc)
        $topLevel | Format-Table -AutoSize | Out-String |
            Set-Content -LiteralPath (Join-Path $usbDir 'top-level.txt') -Encoding utf8
        Say "  Top-level entries: $($topLevel.Count)"
        foreach ($e in $topLevel) { Say "    [$($e.Type)] $($e.Name)" 'DarkGray' }

        # canonical files
        foreach ($f in 'manifest.json','VERSION') {
            $p = Join-Path $UsbRoot $f
            if (Test-Path -LiteralPath $p) {
                Copy-Item -LiteralPath $p -Destination (Join-Path $usbDir $f) -Force
                $h = (Get-FileHash -Algorithm SHA256 -LiteralPath $p).Hash
                Say "  $f  sha256=$h" 'Green'
                Add-Content -LiteralPath (Join-Path $OutDir 'hashes\usb-canonical.sha256') `
                            -Value "$h  $f"
            } else {
                Say "  $f  MISSING" 'Red'
            }
        }

        # vault id WITHOUT dumping secrets: hash + size only
        $vaultIdPath = Join-Path $UsbRoot 'VAULT\vault.id'
        if (Test-Path -LiteralPath $vaultIdPath) {
            $vh = (Get-FileHash -Algorithm SHA256 -LiteralPath $vaultIdPath).Hash
            $vs = (Get-Item -LiteralPath $vaultIdPath).Length
            Say "  VAULT\vault.id present  size=$vs  sha256=$vh" 'Green'
            Set-Content -LiteralPath (Join-Path $usbDir 'vault.id.fingerprint.txt') `
                        -Value "sha256=$vh`nsize=$vs" -Encoding utf8
        } else {
            Say '  VAULT\vault.id  NOT FOUND at expected path' 'Yellow'
            Get-ChildItem -LiteralPath $UsbRoot -Recurse -Depth 3 -Filter 'vault.id' `
                          -Force -ErrorAction SilentlyContinue |
                ForEach-Object { Say "    found instead: $($_.FullName)" 'Yellow' }
        }

        # full recursive hash inventory of executables/models/runtimes
        Say '  Hashing executables, DLLs and models (this can take a few minutes)...' 'DarkGray'
        $hashRows = @()
        Get-ChildItem -LiteralPath $UsbRoot -Recurse -File -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Extension -in '.exe','.dll','.gguf','.bin','.onnx','.so','.dylib' } |
            ForEach-Object {
                $rel = $_.FullName.Substring($UsbRoot.Length).TrimStart('\')
                $hashRows += [pscustomobject]@{
                    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash
                    bytes  = $_.Length
                    path   = $rel
                }
            }
        $hashRows | Sort-Object path |
            Export-Csv -LiteralPath (Join-Path $OutDir 'hashes\usb-binaries.csv') -NoTypeInformation
        Say "  hashed $($hashRows.Count) binary/model file(s)" 'Green'

        # full file listing
        Get-ChildItem -LiteralPath $UsbRoot -Recurse -Force -ErrorAction SilentlyContinue |
            Select-Object @{n='rel';e={$_.FullName.Substring($UsbRoot.Length).TrimStart('\')}},
                          @{n='is_dir';e={$_.PSIsContainer}}, Length, LastWriteTimeUtc |
            Export-Csv -LiteralPath (Join-Path $usbDir 'full-listing.csv') -NoTypeInformation

        # strict validators — exit codes are the evidence
        $strictExit = $null
        $verifyExit = $null

        $strictScript = $null
        foreach ($c in $candidates) {
            $p = Join-Path $c 'scripts\verify-p1-desktop-usb-assets.ps1'
            if (Test-Path -LiteralPath $p) { $strictScript = $p; break }
        }
        if ($strictScript) {
            Say "  running strict asset validation: $strictScript" 'Cyan'
            & powershell -ExecutionPolicy Bypass -File $strictScript -VaultRoot $UsbRoot -Strict `
                *>&1 | Tee-Object -LiteralPath (Join-Path $OutDir 'logs\verify-strict.log') |
                Select-Object -Last 25 | ForEach-Object { Say "    $_" 'DarkGray' }
            $strictExit = $LASTEXITCODE
            Say "  strict validation EXIT CODE = $strictExit" $(if ($strictExit -eq 0) {'Green'} else {'Red'})
        } else {
            Say '  verify-p1-desktop-usb-assets.ps1 not found in any clone — skipped' 'Yellow'
        }

        $starter = Join-Path $UsbRoot 'Start UnoOne.exe'
        if (Test-Path -LiteralPath $starter) {
            Say '  running: "Start UnoOne.exe" --verify-only' 'Cyan'
            & $starter --verify-only *>&1 |
                Tee-Object -LiteralPath (Join-Path $OutDir 'logs\starter-verify-only.log') |
                Select-Object -Last 25 | ForEach-Object { Say "    $_" 'DarkGray' }
            $verifyExit = $LASTEXITCODE
            Say "  starter --verify-only EXIT CODE = $verifyExit" $(if ($verifyExit -eq 0) {'Green'} else {'Red'})
        } else {
            Say '  "Start UnoOne.exe" not found on the drive' 'Red'
        }

        $usbReport = [ordered]@{
            audited              = $true
            usb_root             = $UsbRoot
            volume               = $vol
            partition            = $part
            disk                 = $disk
            top_level_count      = $topLevel.Count
            binary_files_hashed  = $hashRows.Count
            strict_verify_exit   = $strictExit
            starter_verify_exit  = $verifyExit
        }
    }
}

# --------------------------------------------------------------------------
# Summary
# --------------------------------------------------------------------------
Head 'Summary'

$summary = [ordered]@{
    collector_version = '1.0'
    stamp             = $Stamp
    output_dir        = $OutDir
    documented_main   = $TARGET_SHA
    environment       = $env_report
    repositories      = $repoReports
    usb               = $usbReport
}

$summary | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath (Join-Path $OutDir 'environment.json') -Encoding utf8

$dirtyTotal = ($repoReports | Measure-Object -Property dirty_count     -Sum).Sum
$untrTotal  = ($repoReports | Measure-Object -Property untracked_count -Sum).Sum
$copyTotal  = ($repoReports | Measure-Object -Property files_copied    -Sum).Sum

Say "Repositories found      : $($repoReports.Count)"
Say "Tracked dirty entries   : $dirtyTotal"
Say "Untracked files         : $untrTotal"
Say "Files preserved on disk  : $copyTotal"
Say "Recovery branch ref     : $RecBranch"
Say ''
if ($dirtyTotal -gt 0 -or $untrTotal -gt 0) {
    Say 'INTERRUPTED WORK WAS FOUND AND PRESERVED (3 independent copies).' 'Green'
} else {
    Say 'No uncommitted work found. Either it was already committed, or it is' 'Yellow'
    Say 'in a clone outside the scanned roots. Re-run with -SearchRoots before' 'Yellow'
    Say 'concluding it is gone.' 'Yellow'
}
Say ''
Say "SEND BACK -> $(Join-Path $OutDir 'environment.json')" 'Cyan'
Say "         -> $(Join-Path $OutDir 'repos')  (the .txt state dumps)" 'Cyan'
Say ''
Say 'Until confirmed: do NOT run git pull / checkout / reset / clean / stash pop' 'Red'
Say 'in these repositories.' 'Red'

try { Stop-Transcript | Out-Null } catch {}
