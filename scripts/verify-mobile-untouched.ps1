# UnoOne Mobile Golden Baseline Protection Check — PowerShell
# Verifies that no file under android-app/UnoOneAgent/ has changed since the golden baseline.
# Uses both git diff and SHA-256 hash manifest verification.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts/verify-mobile-untouched.ps1
# Exit codes: 0 = PASS, 1 = FAIL (changes detected), 2 = ERROR

param()

$ErrorActionPreference = "Stop"
$GoldenTag = "mobile-golden-baseline-v2"
$ProtectedPath = "android-app/UnoOneAgent/"
$HashManifest = "scripts/MOBILE_GOLDEN_HASHES.txt"

# Step 1: Verify the golden tag exists
$tagList = git tag -l $GoldenTag 2>$null
if ($LASTEXITCODE -ne 0 -or -not $tagList) {
    Write-Error "FAIL: Golden tag '$GoldenTag' not found."
    Write-Error "Run: git tag -a $GoldenTag -m 'Golden baseline' <commit>"
    exit 2
}

# Step 2: Git diff check
$diff = git diff $GoldenTag HEAD -- $ProtectedPath 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Error "FAIL: git diff returned error."
    exit 2
}

if ($diff) {
    Write-Error "FAIL: Changes detected in protected path '$ProtectedPath'"
    Write-Error "Golden baseline tag: $GoldenTag"
    Write-Error ""
    Write-Error "Changed files:"
    git diff --name-only $GoldenTag HEAD -- $ProtectedPath
    Write-Error ""
    Write-Error "The Android application must not be modified during desktop development."
    exit 1
}

# Step 3: Hash manifest verification
if (Test-Path $HashManifest) {
    $canonicalManifest = [IO.Path]::GetTempFileName()
    try {
        node scripts/generate-mobile-golden-hashes.mjs HEAD $canonicalManifest | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Error "FAIL: Could not generate canonical Git-blob hashes."
            exit 2
        }
        $expected = Get-Content -LiteralPath $HashManifest
        $actual = Get-Content -LiteralPath $canonicalManifest
        $differences = Compare-Object -ReferenceObject $expected -DifferenceObject $actual
        if ($differences) {
            Write-Error "FAIL: The protected Git blobs do not match $HashManifest."
            $differences | Select-Object -First 20 | Format-Table -AutoSize
            exit 1
        }
    } finally {
        Remove-Item -LiteralPath $canonicalManifest -Force -ErrorAction SilentlyContinue
    }

    Write-Output "PASS: All $($expected.Count) protected Git blobs match the golden baseline hashes."
} else {
    Write-Warning "WARN: Hash manifest not found at $HashManifest. Skipping hash verification."
}

# Step 4: Verify no files were added or removed
$currentFiles = git ls-tree -r --name-only HEAD -- $ProtectedPath
$baselineFiles = git ls-tree -r --name-only $GoldenTag -- $ProtectedPath

$currentSet = $currentFiles -split "`n" | Where-Object { $_.Trim() -ne '' } | Sort-Object
$baselineSet = $baselineFiles -split "`n" | Where-Object { $_.Trim() -ne '' } | Sort-Object

$added = $currentSet | Where-Object { $_ -notin $baselineSet }
$removed = $baselineSet | Where-Object { $_ -notin $currentSet }

if ($added.Count -gt 0) {
    Write-Error "FAIL: Files added to protected path:"
    $added | ForEach-Object { Write-Error "  + $_" }
    exit 1
}

if ($removed.Count -gt 0) {
    Write-Error "FAIL: Files removed from protected path:"
    $removed | ForEach-Object { Write-Error "  - $_" }
    exit 1
}

Write-Output "PASS: No changes detected in protected path '$ProtectedPath'"
Write-Output "Golden baseline tag: $GoldenTag"
Write-Output "Protected file count: $($currentSet.Count)"
exit 0
