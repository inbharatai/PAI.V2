//! Product-specific adapters connecting the reusable InBharat Harness to Pocket AI.
//!
//! Keep UNOONE/Pocket-AI concerns here. The reusable harness must not import
//! vault-core, Tauri, Android, llama.cpp or other product-specific code.

#![forbid(unsafe_code)]

pub mod coding_task;
pub mod isolation;
pub mod knowledge;
pub mod knowledge_distiller;
pub mod knowledge_retrieval;
pub mod knowledge_service;
pub mod knowledge_verification;
pub mod task_diff;
pub mod task_learning;
pub mod task_ledger;
pub mod task_preview;
pub mod task_workspace;
pub mod task_worktree;

mod llama_local;
mod memory;
mod model_policy;

pub use llama_local::PaiLlamaLocalProvider;
pub use memory::{PaiVaultMemoryProvider, PaiVaultMemoryProviderConfig};
pub use model_policy::{select_model_tier, HostClass, PocketModelTier};
