#!/usr/bin/env bash
# UnoOne Mobile Golden Baseline Protection Check
# Fails if any file under android-app/UnoOneAgent/ has changed since the golden baseline.
#
# Usage: bash scripts/verify-mobile-untouched.sh
# Exit 0 = no changes, Exit 1 = changes detected, Exit 2 = error

set -euo pipefail

GOLDEN_TAG="mobile-golden-baseline-v2"
PROTECTED_PATH="android-app/UnoOneAgent/"

# Verify tag exists
if ! git tag -l "$GOLDEN_TAG" | grep -q "$GOLDEN_TAG"; then
    echo "FAIL: Golden tag '$GOLDEN_TAG' not found."
    echo "Run: git tag -a $GOLDEN_TAG -m 'Golden baseline' <commit>"
    exit 2
fi

# Check for changes
DIFF=$(git diff "$GOLDEN_TAG" HEAD -- "$PROTECTED_PATH" || true)

if [ -z "$DIFF" ]; then
    echo "PASS: No changes detected in protected path '$PROTECTED_PATH'"
    echo "Golden baseline tag: $GOLDEN_TAG"
    exit 0
else
    echo "FAIL: Changes detected in protected path '$PROTECTED_PATH'"
    echo "Golden baseline tag: $GOLDEN_TAG"
    echo ""
    echo "Changed files:"
    git diff --name-only "$GOLDEN_TAG" HEAD -- "$PROTECTED_PATH" | while read -r f; do
        echo "  $f"
    done
    echo ""
    echo "The Android application must not be modified during desktop development."
    echo "If you intended to change mobile code, create a separate integration branch."
    exit 1
fi
