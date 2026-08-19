//! # Pocket AI local-inference selection policy
//!
//! Two decisions that must be correct for the bundled llama.cpp runtime to be
//! trustworthy, expressed as pure functions so they can be proven on any host:
//!
//! * [`preferred_backend`] / [`ranked_backends`] — which acceleration backend
//!   to use, by explicit documented priority rather than probe order.
//! * [`verify_model_identity`] — whether the running model is the one the
//!   package declares, failing **closed** by default.
//!
//! Neither function performs I/O. The desktop crate probes the host (which
//! DLLs exist, what `/v1/models` reported, the on-disk hash) and hands the
//! facts here; this crate decides.

#![forbid(unsafe_code)]

use std::fmt;

/// Acceleration backends Pocket AI can select for llama.cpp.
///
/// This mirrors the desktop `AccelerationBackend` by variant name. It is kept
/// separate (rather than shared) so this crate stays free of desktop
/// dependencies; the desktop side maps between the two in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    Cuda,
    Metal,
    Vulkan,
    Cpu,
}

impl Backend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Backend::Cuda => "CUDA",
            Backend::Metal => "METAL",
            Backend::Vulkan => "VULKAN",
            Backend::Cpu => "CPU",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the host probe found available. `metal` is only meaningful on macOS;
/// callers pass `metal: false` off macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AvailableBackends {
    pub cuda: bool,
    pub metal: bool,
    pub vulkan: bool,
}

/// Backends in descending preference, always ending in CPU.
///
/// Priority is explicit and justified for llama.cpp specifically:
///
/// 1. **Metal** — on Apple Silicon it is the native, always-present path.
/// 2. **CUDA** — the fastest and most mature backend on NVIDIA hardware.
/// 3. **Vulkan** — broad GPU coverage (AMD/Intel/older NVIDIA) but generally
///    slower than CUDA where both exist.
/// 4. **CPU** — the universal fallback, always included last.
///
/// The previous code built this list with `insert(0)` in probe order, so a
/// machine with both CUDA and Vulkan ranked Vulkan first purely because the
/// Vulkan probe ran last. Priority now does not depend on probe order.
pub fn ranked_backends(available: AvailableBackends) -> Vec<Backend> {
    let mut out = Vec::with_capacity(4);
    // Metal only participates when the probe says so (caller gates on macOS).
    if available.metal {
        out.push(Backend::Metal);
    }
    if available.cuda {
        out.push(Backend::Cuda);
    }
    if available.vulkan {
        out.push(Backend::Vulkan);
    }
    out.push(Backend::Cpu);
    out
}

/// The single backend Pocket AI should start with.
pub fn preferred_backend(available: AvailableBackends) -> Backend {
    // ranked_backends always yields at least CPU, so first() is never None.
    ranked_backends(available)[0]
}

/// How strict model-identity verification should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityPolicy {
    /// Production / signed package: a declared hash is required, and a missing
    /// declared hash is itself a failure. Substitution cannot pass unnoticed.
    Strict,
    /// Explicit prototype allowance: if the package declares no hash, identity
    /// falls back to "the server reported a non-empty model id". Chosen only
    /// when the caller has knowingly opted into a hashless prototype drive.
    PrototypeAllowMissingHash,
}

/// Everything the host observed about the running model.
#[derive(Debug, Clone)]
pub struct ModelIdentityFacts<'a> {
    /// `id` reported by the server's `/v1/models`.
    pub reported_model_id: &'a str,
    /// SHA-256 of the model file on disk, if it was hashed. `None` means the
    /// file could not be read/hashed.
    pub disk_sha256: Option<&'a str>,
    /// SHA-256 the package manifest declares for this model, if any.
    pub manifest_sha256: Option<&'a str>,
}

/// Why identity verification failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The server reported no model id at all.
    EmptyModelId,
    /// Strict policy but the manifest declared no hash to check against.
    MissingManifestHashUnderStrict,
    /// A manifest hash was present but the file could not be hashed.
    DiskHashUnavailable,
    /// The on-disk hash did not match the declared hash.
    HashMismatch { expected: String, got: String },
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentityError::EmptyModelId => {
                write!(f, "server did not report a model id")
            }
            IdentityError::MissingManifestHashUnderStrict => write!(
                f,
                "package declares no SHA-256 for this model; refusing under strict policy \
                 (a missing declared hash cannot verify the running model)"
            ),
            IdentityError::DiskHashUnavailable => {
                write!(
                    f,
                    "a manifest hash is declared but the model file could not be hashed"
                )
            }
            IdentityError::HashMismatch { expected, got } => {
                write!(f, "model SHA-256 mismatch: expected {expected} got {got}")
            }
        }
    }
}

/// Decide whether the running model is the declared one.
///
/// Hash comparison is case-insensitive (hex may be upper or lower). Under
/// [`IdentityPolicy::Strict`] a missing manifest hash is a failure, not a skip
/// — that is the fail-open bug this replaces. Under
/// [`IdentityPolicy::PrototypeAllowMissingHash`] a missing manifest hash falls
/// back to the non-empty-id check, but a *present* manifest hash is still
/// enforced in both policies.
pub fn verify_model_identity(
    facts: &ModelIdentityFacts<'_>,
    policy: IdentityPolicy,
) -> Result<(), IdentityError> {
    if facts.reported_model_id.trim().is_empty() {
        return Err(IdentityError::EmptyModelId);
    }

    match facts.manifest_sha256 {
        Some(expected) => match facts.disk_sha256 {
            Some(got) => {
                if expected.eq_ignore_ascii_case(got) {
                    Ok(())
                } else {
                    Err(IdentityError::HashMismatch {
                        expected: expected.to_string(),
                        got: got.to_string(),
                    })
                }
            }
            None => Err(IdentityError::DiskHashUnavailable),
        },
        None => match policy {
            IdentityPolicy::Strict => Err(IdentityError::MissingManifestHashUnderStrict),
            IdentityPolicy::PrototypeAllowMissingHash => Ok(()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- backend precedence -------------------------------------------

    /// The exact regression: CUDA and Vulkan both present must prefer CUDA.
    /// The old insert(0) ordering returned Vulkan here.
    #[test]
    fn cuda_beats_vulkan_when_both_present() {
        let a = AvailableBackends {
            cuda: true,
            vulkan: true,
            metal: false,
        };
        assert_eq!(preferred_backend(a), Backend::Cuda);
        assert_eq!(
            ranked_backends(a),
            vec![Backend::Cuda, Backend::Vulkan, Backend::Cpu]
        );
    }

    #[test]
    fn metal_wins_when_available() {
        let a = AvailableBackends {
            cuda: true,
            vulkan: true,
            metal: true,
        };
        assert_eq!(preferred_backend(a), Backend::Metal);
        assert_eq!(
            ranked_backends(a),
            vec![Backend::Metal, Backend::Cuda, Backend::Vulkan, Backend::Cpu]
        );
    }

    #[test]
    fn vulkan_only_prefers_vulkan_over_cpu() {
        let a = AvailableBackends {
            cuda: false,
            vulkan: true,
            metal: false,
        };
        assert_eq!(preferred_backend(a), Backend::Vulkan);
    }

    #[test]
    fn nothing_available_falls_back_to_cpu() {
        let a = AvailableBackends::default();
        assert_eq!(preferred_backend(a), Backend::Cpu);
        assert_eq!(ranked_backends(a), vec![Backend::Cpu]);
    }

    #[test]
    fn cpu_is_always_present_and_last() {
        for cuda in [false, true] {
            for vulkan in [false, true] {
                for metal in [false, true] {
                    let ranked = ranked_backends(AvailableBackends {
                        cuda,
                        vulkan,
                        metal,
                    });
                    assert_eq!(*ranked.last().unwrap(), Backend::Cpu);
                    assert_eq!(
                        ranked.iter().filter(|b| **b == Backend::Cpu).count(),
                        1,
                        "CPU must appear exactly once"
                    );
                }
            }
        }
    }

    #[test]
    fn ranking_is_a_strict_priority_not_probe_order() {
        // Whatever subset is available, CUDA (if present) always precedes
        // Vulkan (if present).
        for metal in [false, true] {
            let a = AvailableBackends {
                cuda: true,
                vulkan: true,
                metal,
            };
            let r = ranked_backends(a);
            let ci = r.iter().position(|b| *b == Backend::Cuda).unwrap();
            let vi = r.iter().position(|b| *b == Backend::Vulkan).unwrap();
            assert!(ci < vi, "CUDA must rank before Vulkan");
        }
    }

    // ---- model identity ------------------------------------------------

    fn facts<'a>(
        id: &'a str,
        disk: Option<&'a str>,
        manifest: Option<&'a str>,
    ) -> ModelIdentityFacts<'a> {
        ModelIdentityFacts {
            reported_model_id: id,
            disk_sha256: disk,
            manifest_sha256: manifest,
        }
    }

    #[test]
    fn empty_model_id_is_rejected_under_any_policy() {
        for policy in [
            IdentityPolicy::Strict,
            IdentityPolicy::PrototypeAllowMissingHash,
        ] {
            assert_eq!(
                verify_model_identity(&facts("", Some("aa"), Some("aa")), policy),
                Err(IdentityError::EmptyModelId)
            );
            assert_eq!(
                verify_model_identity(&facts("   ", None, None), policy),
                Err(IdentityError::EmptyModelId)
            );
        }
    }

    /// The fail-open bug: no manifest hash used to mean "skip the check".
    /// Strict policy now refuses.
    #[test]
    fn strict_refuses_when_manifest_has_no_hash() {
        assert_eq!(
            verify_model_identity(
                &facts("gemma-12b", Some("abc123"), None),
                IdentityPolicy::Strict
            ),
            Err(IdentityError::MissingManifestHashUnderStrict)
        );
    }

    #[test]
    fn prototype_allows_missing_manifest_hash_with_a_real_id() {
        assert_eq!(
            verify_model_identity(
                &facts("gemma-12b", Some("abc123"), None),
                IdentityPolicy::PrototypeAllowMissingHash
            ),
            Ok(())
        );
    }

    #[test]
    fn matching_hash_passes_case_insensitively_under_both_policies() {
        for policy in [
            IdentityPolicy::Strict,
            IdentityPolicy::PrototypeAllowMissingHash,
        ] {
            assert_eq!(
                verify_model_identity(&facts("gemma-12b", Some("ABCDEF"), Some("abcdef")), policy),
                Ok(())
            );
        }
    }

    #[test]
    fn mismatched_hash_is_rejected_even_in_prototype_mode() {
        // A *present* manifest hash is always enforced, regardless of policy.
        let err = verify_model_identity(
            &facts("gemma-12b", Some("deadbeef"), Some("cafebabe")),
            IdentityPolicy::PrototypeAllowMissingHash,
        )
        .unwrap_err();
        assert!(matches!(err, IdentityError::HashMismatch { .. }));
    }

    #[test]
    fn manifest_hash_present_but_disk_unhashable_is_rejected() {
        assert_eq!(
            verify_model_identity(
                &facts("gemma-12b", None, Some("abcdef")),
                IdentityPolicy::Strict
            ),
            Err(IdentityError::DiskHashUnavailable)
        );
    }

    #[test]
    fn identity_errors_render_without_leaking_content() {
        // Messages must be safe to log: no model bytes, only hashes/ids.
        let e = IdentityError::HashMismatch {
            expected: "aa".into(),
            got: "bb".into(),
        };
        assert!(e.to_string().contains("mismatch"));
    }
}
