#!/usr/bin/env python3
"""
UnoOne Mobile Golden Baseline Protection Check

Compares the current state of android-app/UnoOneAgent/ against the
mobile-golden-baseline-v2 tag. Fails if any file in the protected path
has been modified, added, or deleted.

Usage:
    python3 scripts/verify-mobile-untouched.py

Exit codes:
    0 — No changes detected in the protected path
    1 — Changes detected in the protected path (FAIL)
    2 — Error (tag missing, git not found, etc.)
"""

import subprocess
import sys
import os

GOLDEN_TAG = "mobile-golden-baseline-v2"
PROTECTED_PATH = "android-app/UnoOneAgent/"

def main():
    # Verify the golden tag exists
    result = subprocess.run(
        ["git", "tag", "-l", GOLDEN_TAG],
        capture_output=True, text=True, cwd=os.getcwd()
    )
    if GOLDEN_TAG not in result.stdout:
        print(f"FAIL: Golden tag '{GOLDEN_TAG}' not found. Run: git tag -a {GOLDEN_TAG} -m 'Golden baseline' <commit>")
        return 2

    # Check for changes in the protected path
    result = subprocess.run(
        ["git", "diff", GOLDEN_TAG, "HEAD", "--", PROTECTED_PATH],
        capture_output=True, text=True, cwd=os.getcwd()
    )

    if result.returncode != 0:
        print(f"FAIL: git diff returned error: {result.stderr}")
        return 2

    if result.stdout.strip():
        # Changes detected
        print(f"FAIL: Changes detected in protected path '{PROTECTED_PATH}'")
        print(f"Golden baseline tag: {GOLDEN_TAG}")
        print("Changed files:")
        # Parse the diff to find changed file names
        changed_files = set()
        for line in result.stdout.split('\n'):
            if line.startswith('+++ ') or line.startswith('--- '):
                fname = line[6:].strip()
                if fname.startswith('a/') or fname.startswith('b/'):
                    fname = fname[2:]
                if fname.startswith(PROTECTED_PATH):
                    changed_files.add(fname)
        for f in sorted(changed_files):
            print(f"  {f}")
        print(f"\nThe Android application must not be modified during desktop development.")
        print(f"If you intended to change mobile code, create a separate integration branch.")
        return 1
    else:
        print(f"PASS: No changes detected in protected path '{PROTECTED_PATH}'")
        print(f"Golden baseline tag: {GOLDEN_TAG}")
        return 0

if __name__ == "__main__":
    sys.exit(main())
