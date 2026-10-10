//! JNI transport only. No KDF implementation or caller-selected KDF costs.
//! Calls the PUBLIC vault-core production function unchanged.
use jni::{
    objects::{JByteArray, JObject},
    sys::{jboolean, jbyteArray, jlong},
    JNIEnv,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::{Mutex, MutexGuard},
};
use unoone_vault_core::crypto::{derive_key_encryption_key, SALT_LEN, SPEC_ARGON2_MEMORY_KIB};
use zeroize::Zeroizing;

pub const MAX_PASSWORD_BYTES: usize = 4096;
pub const REQUIRED_AVAILABLE_BYTES: u64 = (SPEC_ARGON2_MEMORY_KIB as u64 + 128 * 1024) * 1024;
static KDF: Mutex<()> = Mutex::new(());

fn admit(available: u64, low_memory: bool) -> Result<MutexGuard<'static, ()>, &'static str> {
    let guard = KDF
        .try_lock()
        .map_err(|_| "Vault KDF is busy or unavailable")?;
    if low_memory || available < REQUIRED_AVAILABLE_BYTES {
        return Err("Insufficient available system RAM for vault KDF; close models/apps and retry");
    }
    Ok(guard)
}

// Read again inside the native single-flight window: the Java snapshot is not
// a reservation. Linux/Android MemAvailable is an estimate, not an OOM guarantee.
fn available_ram() -> Result<u64, &'static str> {
    let text = std::fs::read_to_string("/proc/meminfo")
        .map_err(|_| "Cannot read current native memory availability")?;
    text.lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            if words.next()? != "MemAvailable:" {
                return None;
            }
            words.next()?.parse::<u64>().ok()?.checked_mul(1024)
        })
        .ok_or("Cannot determine current native memory availability")
}

/// Public adapter function for direct Rust contract tests. Production params
/// remain production even in this crate's tests (vault-core is a dependency).
pub fn derive(
    password: &[u8],
    salt: &[u8],
    available: u64,
    low_memory: bool,
) -> Result<Zeroizing<[u8; 32]>, &'static str> {
    if !(1..=MAX_PASSWORD_BYTES).contains(&password.len()) || salt.len() != SALT_LEN {
        return Err("Invalid vault password or salt length");
    }
    let _guard = admit(available, low_memory)?;
    if available_ram()? < REQUIRED_AVAILABLE_BYTES {
        return Err("Insufficient current native RAM for vault KDF");
    }
    let salt: &[u8; SALT_LEN] = salt.try_into().map_err(|_| "Invalid salt length")?;
    derive_key_encryption_key(password, salt)
        .map(Zeroizing::new)
        .map_err(|_| "Vault KDF failed")
}

// Never JNI strings/password logging. Validate lengths BEFORE copies. Rust-owned
// password and output are zeroized on success/error/unwind. JVM array belongs to
// caller. No claim that allocator, JVM/provider copies or registers are erased.
#[no_mangle]
pub extern "system" fn Java_com_unoone_agent_vault_NativeVaultKdf_deriveNative(
    mut env: JNIEnv,
    _this: JObject,
    password: JByteArray,
    salt: JByteArray,
    available: jlong,
    low_memory: jboolean,
) -> jbyteArray {
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<jbyteArray, &'static str> {
        let plen = env
            .get_array_length(&password)
            .map_err(|_| "Invalid password array")?;
        let slen = env
            .get_array_length(&salt)
            .map_err(|_| "Invalid salt array")?;
        if !(1..=MAX_PASSWORD_BYTES as i32).contains(&plen)
            || slen != SALT_LEN as i32
            || available < 0
        {
            return Err("Invalid vault KDF input length or memory snapshot");
        }
        let password = Zeroizing::new(
            env.convert_byte_array(&password)
                .map_err(|_| "Password copy failed")?,
        );
        let salt = env
            .convert_byte_array(&salt)
            .map_err(|_| "Salt copy failed")?;
        let key = derive(&password, &salt, available as u64, low_memory != 0)?;
        env.byte_array_from_slice(key.as_ref())
            .map(|a| a.into_raw())
            .map_err(|_| "Key transfer failed")
    }));
    match result {
        Ok(Ok(array)) => array,
        other => {
            let message = match other {
                Ok(Err(message)) => message,
                _ => "Vault native operation failed",
            };
            // Preserve e.g. a pending VM OutOfMemoryError. No exception crosses FFI.
            if !env.exception_check().unwrap_or(true) {
                let _ = env.throw_new("java/lang/IllegalStateException", message);
            }
            ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_bounds_and_singleflight() {
        assert!(admit(REQUIRED_AVAILABLE_BYTES - 1, false).is_err());
        assert!(admit(u64::MAX, true).is_err());
        let held = admit(u64::MAX, false).unwrap();
        assert!(admit(u64::MAX, false).is_err());
        drop(held);
        assert!(admit(u64::MAX, false).is_ok());
        assert!(derive(&[], &[0; 32], u64::MAX, false).is_err());
        assert!(derive(&[1; 4097], &[0; 32], u64::MAX, false).is_err());
        assert!(derive(&[1], &[0; 31], u64::MAX, false).is_err());
    }
}
