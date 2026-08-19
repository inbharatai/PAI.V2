# 50 — Auto-Launch Test Results

Date: 2026-07-29

| Test | Result | Evidence |
|---|---|---|
| GitHub main/USB audit | `VERIFIED_WORKING` | device, inventory, hashes, and tracked-file comparison recorded |
| Existing strict verifier on USB | `FAILED` | desktop app missing; exit 1 |
| New strict verifier on legacy USB | `FAILED` | schema v2 and desktop app missing; exit 1 |
| PowerShell script parsing | `VERIFIED_WORKING` | all new/updated scripts parsed |
| Synthetic schema-v2 package | `VERIFIED_WORKING` | generated successfully; strict verifier 5/5 |
| Frontend TypeScript build | `VERIFIED_WORKING` | `tsc -b && vite build` |
| Frontend lint | `VERIFIED_WORKING` | `oxlint` |
| Rust formatting | `VERIFIED_WORKING` | `cargo fmt --all --check` |
| Cargo metadata | `VERIFIED_WORKING` | workspace manifests resolve |
| Strict validator Rust tests | `VERIFIED_WORKING` | 3/3: valid package, tampered executable, and traversal rejection |
| Full native Rust workspace compile | `BLOCKED_BY_ENVIRONMENT` | OS error 4551 from enforced Application Control |
| Android vault unit tests | `VERIFIED_WORKING` | `:vault:testDebugUnitTest` passed |
| Android app compilation | `VERIFIED_WORKING` | `:app:compileDebugKotlin` passed |
| Android app/vault lint | `VERIFIED_WORKING` | no new lint issues |
| Windows Dock insertion | `BLOCKED_BY_ENVIRONMENT` | binary cannot be built/signed on this host |
| Start UnoOne fallback | `BLOCKED_BY_ENVIRONMENT` | binary cannot be built/signed on this host |
| Real model inference | `BLOCKED_BY_ENVIRONMENT` | no Power binary; WDAC blocks unsigned runtime |
| Removal during model/recording | `IMPLEMENTED_NOT_TESTED` | cleanup source exists; physical test pending |
| Ordinary/tampered USB rejection | `IMPLEMENTED_UNIT_TESTED` | strict validator tamper/traversal tests pass; physical host test pending |
| Android physical attachment | `BLOCKED_BY_ENVIRONMENT` | target phone unavailable |

Decision: `WINDOWS_AUTO_LAUNCH: NOT_VERIFIED`

No physical auto-launch claim is made.
