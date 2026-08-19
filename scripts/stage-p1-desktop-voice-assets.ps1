#requires -Version 5.1
<#
.SYNOPSIS
    Retired legacy Pocket AI voice-only staging entry point.

.DESCRIPTION
    The old implementation copied only the two entry-point executables, omitted
    required DLLs and Piper data, and rewrote the schema-v1 manifest in place.
    That could leave the pen drive looking configured while voice could not run.

    Use build-pocket-ai-windows.ps1 with -VoiceBundleRoot instead. It stages the
    complete voice runtime, models, applications, and schema-v2 manifest in one
    backed-up transaction and rolls everything back if strict verification fails.
#>
[CmdletBinding()]
param(
    [string]$VaultRoot = "D:\UNOONE",
    [string]$WhisperBin = "",
    [string]$WhisperModel = "",
    [string]$PiperBin = "",
    [string]$PiperModel = ""
)

throw @"
This legacy staging script is retired because it cannot produce a complete,
strictly verified Pocket AI package.

Use:
  .\scripts\build-pocket-ai-windows.ps1 -VaultRoot "$VaultRoot" -SkipBuild `
    -PowerBinaryPath <UnoOnePower.exe> `
    -DockBinaryPath <UnoOneDock.exe> `
    -StarterBinaryPath <Start UnoOne.exe> `
    -VoiceBundleRoot <bundle-root>
"@
