//! Product-specific adapters connecting the reusable InBharat Harness to Pocket AI.
//!
//! Keep UNOONE/Pocket-AI concerns here. The reusable harness must not import
//! vault-core, Tauri, Android, llama.cpp or other product-specific code.

#![forbid(unsafe_code)]

mod llama_local;
mod memory;
mod model_policy;

pub use llama_local::PaiLlamaLocalProvider;
pub use memory::{PaiVaultMemoryProvider, PaiVaultMemoryProviderConfig};
pub use model_policy::{select_model_tier, HostClass, PocketModelTier};
