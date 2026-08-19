param(
  [Parameter(Mandatory=$true)][string]$AudioCppSource,
  [string]$BuildDir = "build-pocket-ai-windows"
)
$ErrorActionPreference='Stop'
$root=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$source=(Resolve-Path -LiteralPath $AudioCppSource).Path
if(-not(Test-Path (Join-Path $source '.git'))){throw "audio.cpp source must be an exact Git checkout: $source"}
$build=Join-Path $root $BuildDir
cmake -S $root -B $build -DCMAKE_BUILD_TYPE=Release `
  -DIBAUDIO_BUILD_TESTS=OFF `
  -DIBAUDIO_ENABLE_TEST_FIXTURE_MODELS=OFF `
  -DIBAUDIO_ENABLE_EXPERIMENTAL_RESEARCH_MODULES=OFF `
  -DIBAUDIO_ENABLE_AUDIO_CPP_ADAPTER=ON `
  "-DIBAUDIO_AUDIO_CPP_SOURCE_DIR=$source"
if($LASTEXITCODE -ne 0){throw 'configure failed'}
cmake --build $build --config Release --parallel
if($LASTEXITCODE -ne 0){throw 'build failed'}
$cli=Join-Path $build 'Release\ibaudio.exe'; if(-not(Test-Path $cli)){$cli=Join-Path $build 'ibaudio.exe'}
if(-not(Test-Path $cli)){throw 'ibaudio.exe not produced'}
$status=& $cli audio-cpp-status --json | ConvertFrom-Json
if(-not $status.adapter_compiled){throw "audio.cpp provenance adapter is not compiled: $($status.reason)"}
$models=& $cli models --json
if($models -match 'reference-asr|reference-tts|deferred-kws'){throw 'production binary exposes fixture/deferred engines'}
Write-Host "PASS: Pocket AI Windows InBharat Audio production provenance build: $cli"
Write-Host 'NEXT: run real audio.cpp ASR/TTS acceptance with the actual models.'
