@echo off
:: UnoOne Power — Windows USB Launcher
:: Detects Pocket USB drive and starts UnoOne Power desktop app

title UnoOne Power - Pocket USB Launcher
echo.
echo  ╔══════════════════════════════════════════╗
echo  ║   UnoOne Power - Private AI Workstation  ║
echo  ║   Pocket USB Launcher for Windows        ║
echo  ╚══════════════════════════════════════════╝
echo.

:: Check for Pocket USB drive
set "VAULT_FOUND=0"
set "VAULT_DRIVE="

echo [1/4] Scanning for UnoOne Pocket USB drive...

for %%D in (D E F G H I J K L M N O P Q R S T U V W X Y Z) do (
    if exist "%%D:\UNOONE\VAULT\identity\vault.id" (
        set "VAULT_FOUND=1"
        set "VAULT_DRIVE=%%D:"
        echo       Found Pocket USB on drive %%D:
    )
)

if "%VAULT_FOUND%"=="0" (
    echo.
    echo  [ERROR] No UnoOne Pocket USB drive detected.
    echo.
    echo  Please connect your Pocket USB drive and try again.
    echo  The drive should contain: \UNOONE\VAULT\identity\vault.id
    echo.
    pause
    exit /b 1
)

echo.
echo  Using Pocket USB: %VAULT_DRIVE%\UNOONE
echo.

:: Check if UnoOne Power is installed
echo [2/4] Checking UnoOne Power installation...

set "UNOONE_DIR=%LOCALAPPDATA%\UnoOne\Power"
if not exist "%UNOONE_DIR%\unoone-power.exe" (
    echo  [WARN] UnoOne Power not found at %UNOONE_DIR%
    echo  Looking for portable installation...

    :: Check USB for portable installation
    set "UNOONE_DIR=%VAULT_DRIVE%\UNOONE\RUNTIMES\windows"
    if not exist "%UNOONE_DIR%\unoone-power.exe" (
        echo.
        echo  [ERROR] UnoOne Power executable not found.
        echo.
        echo  Please install UnoOne Power or place it in:
        echo  %VAULT_DRIVE%\UNOONE\RUNTIMES\windows\
        echo.
        pause
        exit /b 1
    )
)

echo  Found: %UNOONE_DIR%\unoone-power.exe
echo.

:: Check model files
echo [3/4] Checking Gemma 4 model files...

set "MODEL_DIR=%VAULT_DRIVE%\UNOONE\MODELS\gemma4-12b-q4-gguf"
if exist "%MODEL_DIR%" (
    echo  Model directory found: %MODEL_DIR%
    dir /b "%MODEL_DIR%\*.gguf" 2>nul | findstr /i "gemma" >nul
    if errorlevel 1 (
        echo  [WARN] No .gguf model files found in model directory.
    ) else (
        echo  Model files detected.
    )
) else (
    echo  [WARN] Model directory not found: %MODEL_DIR%
    echo  Models will need to be downloaded before first use.
)

echo.

:: Launch UnoOne Power
echo [4/4] Launching UnoOne Power...
echo.

cd /d "%UNOONE_DIR%"
start "" "%UNOONE_DIR%\unoone-power.exe"

echo  UnoOne Power launched successfully.
echo.
echo  Keep your Pocket USB connected while using UnoOne Power.
echo  Closing this window will NOT close UnoOne Power.
echo.
timeout /t 5 /nobreak >nul