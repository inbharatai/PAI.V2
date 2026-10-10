//! Google provider boundary. No model tool registration and no authority from provider text.
//! Tokens and native grants are local-only; none implement Debug or public UI serialization.
pub mod google;
pub mod guardian;
pub mod oauth;
pub mod store;
pub mod types;
pub use types::*;
#[cfg(test)]
mod tests;

pub type Result<T> = std::result::Result<T, String>;
pub(crate) fn ensure(ok: bool, error: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(error.into())
    }
}
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
