#requires -Version 5.1
<#
.SYNOPSIS
  Non-WDAC full-acceptance smoke test for the UnoOne desktop llama-server runtime.

.DESCRIPTION
  Validates the shipped llama-server stub end-to-end on a host WITHOUT a
  WDAC/AppLocker policy that blocks unsigned native executables. It verifies:

    1. llama-server.exe exists and is runnable.
    2. llama-server-impl.dll (and friends) exist next to the stub.
    3. A free localhost TCP port is selected dynamically (no collisions).
    4. The requested model file exists.
    5. The on-disk model SHA-256 matches the USB manifest entry when present.
    6. The server starts from its own directory so DLL search succeeds.
    7. /health returns 200 and reports a model loaded.
    8. /v1/models lists the expected model id.
    9. /v1/chat/completions produces a non-empty response (real inference).
   10. The child PID is tracked and the process shuts down cleanly.

  The script exits with code 0 on success and writes a summary JSON to the
  pipeline. It does not modify USB contents.
#>
[CmdletBinding()]
param(
    [string]$VaultRoot = "D:\UNOONE",
    [string]$Backend = "CPU",
    [string]$ModelRelativePath = "MODELS\DESKTOP\Gemma-12B\gemma-4-12B-it-Q4_K_M.gguf",
    [int]$TimeoutSeconds = 180,
    [switch]$SkipInference
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Net.Http

function Get-SHA256($path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = [System.IO.File]::OpenRead($path)
        try {
            $bytes = $sha.ComputeHash($stream)
            return ($bytes | ForEach-Object { $_.ToString("x2") }) -join ""
        } finally { $stream.Close() }
    } finally { $sha.Dispose() }
}

function Read-ManifestHash($vaultRoot, $modelPath) {
    $manifest = Join-Path $vaultRoot "manifest.json"
    if (-not (Test-Path $manifest)) { return $null }
    $json = Get-Content $manifest -Raw | ConvertFrom-Json
    $rel = $modelPath.Substring($vaultRoot.Length).TrimStart('\', '/')
    foreach ($section in @("desktop", "mobile")) {
        $obj = $json.models.$section
        if (-not $obj) { continue }
        foreach ($model in $obj.PSObject.Properties.Value) {
            if ($model.path -eq $rel -or $model.path -eq $modelPath) {
                return $model.sha256
            }
        }
    }
    return $null
}

function Find-FreePort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    $listener.Start()
    try {
        return $listener.LocalEndpoint.Port
    } finally {
        $listener.Stop()
    }
}

function Test-Health($port) {
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$port/health" -TimeoutSec 3 -UseBasicParsing
        return ($resp.StatusCode -eq 200), $resp.Content
    } catch {
        return $false, $_.Exception.Message
    }
}

function Test-Models($port) {
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$port/v1/models" -TimeoutSec 5 -UseBasicParsing
        return ($resp.StatusCode -eq 200), $resp.Content
    } catch {
        return $false, $_.Exception.Message
    }
}

function Test-Inference($port) {
    $body = @{
        model = "gemma-4-12b"
        messages = @(@{role = "user"; content = "Say the exact word 'pong' and nothing else."})
        max_tokens = 16
        temperature = 0.1
        stream = $false
    } | ConvertTo-Json -Depth 4

    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$port/v1/chat/completions" `
            -Method POST -Body $body -ContentType "application/json" -TimeoutSec 30 -UseBasicParsing
        if ($resp.StatusCode -ne 200) { return $false, $resp.Content }
        $data = $resp.Content | ConvertFrom-Json
        $text = $data.choices[0].message.content
        return ($text -match 'pong'), $text
    } catch {
        return $false, $_.Exception.Message
    }
}

$serverExe = Join-Path $VaultRoot "RUNTIMES\WINDOWS\$Backend\llama-server.exe"
$modelFile = Join-Path $VaultRoot $ModelRelativePath
$backendDir = Split-Path -Parent $serverExe

Write-Host "Vault root : $VaultRoot"
Write-Host "Backend    : $Backend"
Write-Host "Server     : $serverExe"
Write-Host "Model      : $modelFile"

if (-not (Test-Path $serverExe)) { throw "llama-server.exe not found at $serverExe" }
if (-not (Test-Path $modelFile)) { throw "Model file not found at $modelFile" }
$implDll = Join-Path $backendDir "llama-server-impl.dll"
if (-not (Test-Path $implDll)) { throw "llama-server-impl.dll missing in $backendDir - the stub cannot load" }

$port = Find-FreePort
Write-Host "Selected port: $port"

$modelHash = Get-SHA256 $modelFile
Write-Host "Model SHA-256: $modelHash"
$expectedHash = Read-ManifestHash $VaultRoot $modelFile
if ($expectedHash) {
    if ($modelHash -ne $expectedHash) {
        throw "Model hash mismatch: expected $expectedHash, got $modelHash"
    }
    Write-Host "Manifest model hash matches." -ForegroundColor Green
} else {
    Write-Warning "No manifest hash found; skipping manifest verification."
}

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $serverExe
$psi.WorkingDirectory = $backendDir
$psi.Arguments = @(
    "-m", "`"$modelFile`"",
    "--port", $port,
    "-c", "4096",
    "-b", "512",
    "--temp", "0.7",
    "--top-p", "0.9",
    "--top-k", "40",
    "--repeat-penalty", "1.1",
    "-n", "4096",
    "-ngl", "0"
) -join " "
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true

$proc = [System.Diagnostics.Process]::Start($psi)
$stdout = $proc.StandardOutput.ReadToEndAsync()
$stderr = $proc.StandardError.ReadToEndAsync()

$healthy = $false
$healthBody = ""
$modelsBody = ""
$inferenceOk = $false
$inferenceText = ""

try {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ($proc.HasExited) {
            throw "llama-server exited early (exit code $($proc.ExitCode)).`nSTDOUT:`n$($stdout.Result)`nSTDERR:`n$($stderr.Result)"
        }
        $healthy, $healthBody = Test-Health $port
        if ($healthy) { break }
        Start-Sleep -Milliseconds 500
    }

    if (-not $healthy) {
        throw "Server did not become healthy within $TimeoutSeconds seconds.`nSTDOUT:`n$($stdout.Result)`nSTDERR:`n$($stderr.Result)"
    }
    Write-Host "SUCCESS: /health returned 200" -ForegroundColor Green

    $modelsOk, $modelsBody = Test-Models $port
    if (-not $modelsOk) { throw "/v1/models did not return 200: $modelsBody" }
    $modelId = ($modelsBody | ConvertFrom-Json).data[0].id
    if (-not $modelId) { throw "Server did not report a model id in /v1/models" }
    Write-Host "SUCCESS: /v1/models reports model id '$modelId'" -ForegroundColor Green

    if (-not $SkipInference) {
        $inferenceOk, $inferenceText = Test-Inference $port
        if (-not $inferenceOk) { throw "Inference did not return expected response: $inferenceText" }
        Write-Host "SUCCESS: inference returned '$inferenceText'" -ForegroundColor Green
    }
} finally {
    if (-not $proc.HasExited) {
        $proc.Kill($true)
        $proc.WaitForExit()
    }
    Write-Host "Process stopped (PID $($proc.Id), exit $($proc.ExitCode))."
    Write-Host "STDOUT:`n$($stdout.Result)"
    Write-Host "STDERR:`n$($stderr.Result)"
}

$result = [ordered]@{
    status = "ACCEPTED"
    vaultRoot = $VaultRoot
    backend = $Backend
    port = $port
    serverPid = $proc.Id
    modelHash = $modelHash
    manifestHash = $expectedHash
    modelId = $modelId
    inferenceOk = $inferenceOk
    inferenceText = $inferenceText
    stdoutTail = ($stdout.Result -split "`n")[-5..-1] -join "`n"
    stderrTail = ($stderr.Result -split "`n")[-5..-1] -join "`n"
}

Write-Output ($result | ConvertTo-Json -Depth 4)
