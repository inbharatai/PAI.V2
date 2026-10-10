//! Pure policy only: no filesystem, network, device probe, model loader or cloud.
//! Serialized DTOs are inert. Native trust wrappers must not be reconstructed
//! from workflow JSON. See docs/MODEL_ADMISSION_CONTRACT.md for adapter duties.
//!
//! Trust wrappers and execution state cannot be hydrated as authority:
//! ```compile_fail
//! let _: unoone_model_admission::VerifiedCandidate = serde_json::from_str("{}").unwrap();
//! ```
//! ```compile_fail
//! let _: unoone_model_admission::ValidatedQualification = serde_json::from_str("{}").unwrap();
//! ```
//! ```compile_fail
//! let _: unoone_model_admission::NativePreflight = serde_json::from_str("{}").unwrap();
//! ```
//! ```compile_fail
//! let _: unoone_model_admission::NativePolicyGrant = serde_json::from_str("{}").unwrap();
//! ```
//! ```compile_fail
//! let _: unoone_model_admission::Provisioner = serde_json::from_str("{}").unwrap();
//! ```
pub mod admission;
pub mod dto;
pub mod lifecycle;
pub mod policy;
pub mod router;
pub mod trust;
pub use admission::*;
pub use dto::*;
pub use lifecycle::*;
pub use policy::*;
pub use router::*;
pub use trust::*;
