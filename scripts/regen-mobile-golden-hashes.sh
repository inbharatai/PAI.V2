#!/usr/bin/env bash
# Re-baseline the mobile golden protection.
#
# The old scheme pinned a commit SHA inside mobile-protection.yml, so every
# legitimate mobile change required editing a workflow file to re-baseline. A
# gate that fires on correct work gets disabled by whoever tires of it first,
# which is the worst possible outcome for a safety mechanism.
#
# This scheme pins the git TREE hash of the protected path in a committed file.
# Re-baselining is an ordinary, reviewable commit: the tree hash cannot be
# changed without changing the protected files, and the protected files cannot
# be changed without updating this pointer, so an unreviewed drift still fails.
#
# Run from the repository root after an intentional mobile change.
set -euo pipefail

PROTECTED_PATH="android-app/UnoOneAgent"
TREE=$(git rev-parse "HEAD:${PROTECTED_PATH}")

{
  echo "# Pocket AI — mobile golden baseline hash manifest"
  echo "# Protected path: ${PROTECTED_PATH}/"
  echo "# Protected tree: ${TREE}"
  echo "# Regenerate with: scripts/regen-mobile-golden-hashes.sh"
  echo "# One SHA-256 per protected file, LF-canonical, verified by sha256sum --check --strict"
  # Hash the git BLOB for each path, not the working-tree file: with
  # core.autocrlf=true a Windows checkout materialises CRLF bytes, while CI
  # checks out LF and sha256sum --check --strict fails on every entry. Blob
  # bytes are the canonical LF bytes every checkout yields.
  git ls-tree -r --name-only HEAD -- "${PROTECTED_PATH}/" | sort | while read -r f; do
    printf '%s  %s\n' "$(git cat-file blob "$(git rev-parse "HEAD:${f}")" | sha256sum | cut -d' ' -f1)" "$f"
  done
} > scripts/MOBILE_GOLDEN_HASHES.txt

echo "${TREE}" > scripts/MOBILE_PROTECTED_TREE

echo "Re-baselined mobile protection:"
echo "  protected tree : ${TREE}"
echo "  files hashed   : $(grep -c '^[0-9a-f]\{64\}  ' scripts/MOBILE_GOLDEN_HASHES.txt)"
echo
echo "Commit scripts/MOBILE_GOLDEN_HASHES.txt and scripts/MOBILE_PROTECTED_TREE together."
