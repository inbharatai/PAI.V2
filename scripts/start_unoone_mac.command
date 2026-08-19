#!/bin/bash
# UnoOne Power — macOS USB Launcher
# Detects Pocket USB drive and starts UnoOne Power desktop app

set -e

echo ""
echo "  ╔══════════════════════════════════════════╗"
echo "  ║   UnoOne Power - Private AI Workstation  ║"
echo "  ║   Pocket USB Launcher for macOS           ║"
echo "  ╚══════════════════════════════════════════╝"
echo ""

# Check for Pocket USB drive
echo "[1/4] Scanning for UnoOne Pocket USB drive..."

VAULT_DRIVE=""
for mount_point in /Volumes/PAI /Volumes/UNOONE /Volumes/PocketUSB; do
    if [ -f "$mount_point/UNOONE/VAULT/identity/vault.id" ]; then
        VAULT_DRIVE="$mount_point"
        echo "      Found Pocket USB at $mount_point"
        break
    fi
done

if [ -z "$VAULT_DRIVE" ]; then
    # Also scan for any mounted volume with the vault ID
    for mount_point in /Volumes/*; do
        if [ -f "$mount_point/UNOONE/VAULT/identity/vault.id" ]; then
            VAULT_DRIVE="$mount_point"
            echo "      Found Pocket USB at $mount_point"
            break
        fi
    done
fi

if [ -z "$VAULT_DRIVE" ]; then
    echo ""
    echo "  [ERROR] No UnoOne Pocket USB drive detected."
    echo ""
    echo "  Please connect your Pocket USB drive and try again."
    echo "  The drive should contain: /UNOONE/VAULT/identity/vault.id"
    echo ""
    read -p "Press Enter to exit..."
    exit 1
fi

echo ""
echo "  Using Pocket USB: $VAULT_DRIVE/UNOONE"
echo ""

# Check if UnoOne Power is installed
echo "[2/4] Checking UnoOne Power installation..."

UNOONE_APP="/Applications/UnoOne Power.app"
if [ ! -d "$UNOONE_APP" ]; then
    echo "  [WARN] UnoOne Power not found in /Applications"
    echo "  Looking for portable installation..."

    UNOONE_BIN="$VAULT_DRIVE/UNOONE/RUNTIMES/macos/unoone-power"
    if [ ! -f "$UNOONE_BIN" ]; then
        echo ""
        echo "  [ERROR] UnoOne Power executable not found."
        echo ""
        echo "  Please install UnoOne Power or place it in:"
        echo "  $VAULT_DRIVE/UNOONE/RUNTIMES/macos/"
        echo ""
        read -p "Press Enter to exit..."
        exit 1
    fi
    UNOONE_CMD="$UNOONE_BIN"
else
    echo "  Found: $UNOONE_APP"
    UNOONE_CMD="open \"$UNOONE_APP\""
fi

echo ""

# Check model files
echo "[3/4] Checking Gemma 4 model files..."

MODEL_DIR="$VAULT_DRIVE/UNOONE/MODELS/gemma4-12b-q4-gguf"
if [ -d "$MODEL_DIR" ]; then
    echo "  Model directory found: $MODEL_DIR"
    model_count=$(find "$MODEL_DIR" -name "*.gguf" 2>/dev/null | wc -l)
    if [ "$model_count" -gt 0 ]; then
        echo "  Model files detected ($model_count .gguf files)."
    else
        echo "  [WARN] No .gguf model files found in model directory."
    fi
else
    echo "  [WARN] Model directory not found: $MODEL_DIR"
    echo "  Models will need to be downloaded before first use."
fi

echo ""

# Launch UnoOne Power
echo "[4/4] Launching UnoOne Power..."
echo ""

eval "$UNOONE_CMD"

echo "  UnoOne Power launched successfully."
echo ""
echo "  Keep your Pocket USB connected while using UnoOne Power."
echo ""
sleep 3