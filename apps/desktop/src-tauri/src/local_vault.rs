//! Independent local vault lifecycle, using the existing vault-core format/crypto.
//! No model validation is implied by local storage readiness.
use crate::local_install;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use unoone_vault_core::{vault::VaultCreateResult, Vault};

pub static OPERATIONS: Mutex<()> = Mutex::new(());
const MARKER: &str = "local-install.json";

/// Zero Rust-owned secret input buffers on every exit; never Debug/Serialize.
pub struct Secret(pub Vec<u8>);
impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        unoone_vault_core::crypto::secure_zero(&mut self.0);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    version: u32,
    vault_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    New,
    Locked,
    Interrupted,
}

pub fn inspect(root: &Path, pending: &Path) -> Result<(State, String), String> {
    local_install::checked_path(root)?;
    local_install::checked_path(pending)?;
    match (local_install::present(root), local_install::present(pending)) {
        (true, true) => Err("Both active and interrupted local vaults exist. Nothing was replaced; preserve both for manual recovery.".into()),
        (true, false) => Ok((State::Locked, open(root)?.vault_id().unwrap_or_default().to_owned())),
        (false, true) => { local_install::private_owner(pending)?; Ok((State::Interrupted, String::new())) },
        (false, false) => Ok((State::New, String::new())),
    }
}

pub fn open(root: &Path) -> Result<Vault, String> {
    // Check only the private vault subtree and installation identity, not GBs of
    // future model/runtime assets. They retain their separate verification gate.
    local_install::private_owner(root)?;
    local_install::checked_path(&root.join(MARKER))?;
    let marker = root.join(MARKER);
    let len = fs::metadata(&marker)
        .map_err(|_| "Local vault identity is missing; no automatic re-creation".to_string())?
        .len();
    if len > 4096 {
        return Err("Local vault identity exceeds limit".into());
    }
    let identity: Identity = serde_json::from_slice(&fs::read(marker).map_err(|e| e.to_string())?)
        .map_err(|_| "Malformed local vault identity".to_string())?;
    if identity.version != 1 || uuid::Uuid::parse_str(&identity.vault_id).is_err() {
        return Err("Unsupported local vault identity".into());
    }
    local_install::checked_tree(&root.join("VAULT"))?;
    for file in ["header_a.json", "header_b.json"] {
        if let Ok(m) = fs::metadata(root.join("VAULT/header").join(file)) {
            if m.len() > 65536 {
                return Err("Local vault header exceeds limit".into());
            }
        }
    }
    let id_path = root.join("VAULT/identity/vault.id");
    if fs::metadata(&id_path).map_err(|e| e.to_string())?.len() > 128 {
        return Err("Invalid vault ID size".into());
    }
    if fs::read_to_string(id_path)
        .map_err(|e| e.to_string())?
        .trim()
        != identity.vault_id
    {
        return Err("Local vault identities disagree; no automatic migration".into());
    }
    let vault = Vault::open(root).map_err(|e| format!("Cannot open local vault: {e}"))?;
    if vault.vault_id() != Some(identity.vault_id.as_str()) {
        return Err("Header and local identity disagree".into());
    }
    Ok(vault)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = path; // std does not offer portable directory fsync on Windows.
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn publish(root: &Path, pending: &Path) -> Result<(), String> {
    local_install::checked_path(root)?;
    if local_install::present(root) {
        return Err("Refusing to replace an existing local vault".into());
    }
    fs::rename(pending, root).map_err(|e| {
        format!("Local vault publication interrupted; preserve pending directory: {e}")
    })?;
    sync_directory(root.parent().ok_or("Missing parent")?)
}

pub fn create(root: &Path, pending: &Path, password: &[u8]) -> Result<VaultCreateResult, String> {
    if password.len() < 8 || password.len() > 4096 {
        return Err("Password must contain 8–4096 UTF-8 bytes".into());
    }
    if inspect(root, pending)?.0 != State::New {
        return Err(
            "A vault or interrupted creation already exists. Use unlock/recovery, not create."
                .into(),
        );
    }
    // Exclusive mkdir claims first creation. A crash retains pending, never
    // silently starts a second identity or deletes the first attempt.
    let parent = root.parent().ok_or("Missing local data parent")?;
    local_install::checked_path(parent)?;
    if !local_install::present(parent) {
        local_install::private_dir(parent)?;
    }
    if local_install::present(pending) {
        return Err("Creation already in progress".into());
    }
    local_install::claim_private_dir(pending)?;
    let created = Vault::create(pending, password)
        .map_err(|e| format!("Creation interrupted; existing files retained: {e}"))?;
    // Protect the vault subtree, also relied upon by open()/backup validation.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(pending.join("VAULT"), fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let marker = Identity {
        version: 1,
        vault_id: created.vault_id.clone(),
    };
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(pending.join(MARKER))
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(&marker).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    // Validate the identity before publication. Creation already authenticated
    // the header; reopening here is structural, never a fake unlock.
    open(pending)?;
    for path in local_install::checked_tree(pending)? {
        fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    sync_directory(pending)?;
    publish(root, pending)?;
    Ok(created)
}

/// Explicit recovery of a fully-written staging directory; authentication is
/// required. Incomplete/malformed staging stays untouched for manual recovery.
pub fn resume(root: &Path, pending: &Path, password: &[u8]) -> Result<(), String> {
    if inspect(root, pending)?.0 != State::Interrupted {
        return Err("No interrupted creation to resume".into());
    }
    let mut vault = open(pending)?;
    vault
        .unlock(password)
        .map_err(|_| "Could not authenticate interrupted vault".to_string())?;
    vault.lock().map_err(|e| e.to_string())?;
    drop(vault);
    publish(root, pending)
}

/// Encrypted same-device safety copy, never a second active replica. No model
/// files or raw key extraction. Caller must hold OPERATIONS and a locked state.
pub fn backup(root: &Path, destination: &Path) -> Result<(), String> {
    open(root)?;
    if local_install::present(destination) {
        return Err("Backup destination already exists".into());
    }
    let files = local_install::checked_tree(&root.join("VAULT"))?;
    let total = files.iter().try_fold(0u64, |sum, f| {
        sum.checked_add(fs::metadata(f).map_err(|e| e.to_string())?.len())
            .ok_or_else(|| "Backup too large".to_string())
    })?;
    if total > 2 * 1024 * 1024 * 1024 {
        return Err("Backup exceeds 2 GiB safety limit; use an offline filesystem backup".into());
    }
    local_install::claim_private_dir(destination)?;
    // Include empty structural directories: vault-core needs them after restore.
    fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
        local_install::private_dir(to)?;
        for e in fs::read_dir(from).map_err(|e| e.to_string())? {
            let e = e.map_err(|e| e.to_string())?;
            let dest = to.join(e.file_name());
            if e.file_type().map_err(|e| e.to_string())?.is_dir() {
                copy_dir(&e.path(), &dest)?;
            } else {
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dest)
                    .map_err(|e| e.to_string())?;
                let mut input = fs::File::open(e.path()).map_err(|e| e.to_string())?;
                std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
                output.sync_all().map_err(|e| e.to_string())?;
            }
        }
        sync_directory(to)
    }
    copy_dir(&root.join("VAULT"), &destination.join("VAULT"))?;
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join(MARKER))
        .map_err(|e| e.to_string())?;
    f.write_all(&fs::read(root.join(MARKER)).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    open(destination)?;
    let mut complete = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("BACKUP_COMPLETE"))
        .map_err(|e| e.to_string())?;
    complete
        .write_all(b"Encrypted backup only; never activate as a second writable replica.\n")
        .map_err(|e| e.to_string())?;
    complete.sync_all().map_err(|e| e.to_string())?;
    sync_directory(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unoone_vault_core::{Record, RecordType};
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (root, pending) = local_install::roots(dir.path()).unwrap();
        (dir, root, pending)
    }
    #[test]
    fn independent_crypto_wrong_key_recovery_and_encrypted_backup() {
        let (dir, root, pending) = fixture();
        assert_eq!(inspect(&root, &pending).unwrap().0, State::New);
        let created = create(&root, &pending, b"password-one").unwrap();
        let (other_dir, other, other_pending) = fixture();
        let other_created = create(&other, &other_pending, b"password-one").unwrap();
        assert_ne!(created.vault_id, other_created.vault_id);
        let mut a = open(&root).unwrap();
        let mut b = open(&other).unwrap();
        assert!(a.unlock(b"wrong-password").is_err());
        assert!(!a.is_unlocked());
        a.unlock(b"password-one").unwrap();
        b.unlock(b"password-one").unwrap();
        assert_ne!(a.master_key(), b.master_key());
        let record = Record::new(RecordType::Memory, "DESKTOP", "local-test");
        a.write_record(record.clone(), b"private local record")
            .unwrap();
        a.lock().unwrap();
        let mut recovered = open(&root).unwrap();
        recovered
            .unlock_with_recovery(&created.recovery_phrase)
            .unwrap();
        assert_eq!(
            recovered.read_record(&record.record_id).unwrap().1,
            b"private local record"
        );
        recovered.lock().unwrap();
        let copy = dir.path().join("backup");
        backup(&root, &copy).unwrap();
        assert!(copy.join("BACKUP_COMPLETE").is_file());
        let mut restored = open(&copy).unwrap();
        restored.unlock(b"password-one").unwrap();
        assert_eq!(
            restored.read_record(&record.record_id).unwrap().1,
            b"private local record"
        );
        assert!(backup(&root, &copy).is_err());
        assert!(create(&root, &pending, b"another-password").is_err());
        assert_eq!(
            open(&root).unwrap().vault_id(),
            Some(created.vault_id.as_str())
        );
        for file in local_install::checked_tree(&root).unwrap() {
            let bytes = fs::read(file).unwrap();
            assert!(!bytes
                .windows(b"private local record".len())
                .any(|w| w == b"private local record"));
            assert!(!bytes
                .windows(b"password-one".len())
                .any(|w| w == b"password-one"));
        }
        drop(other_dir);
    }
    #[test]
    fn interrupted_create_requires_authentication_and_never_overwrites() {
        let (_dir, root, pending) = fixture();
        create(&root, &pending, b"original-password").unwrap();
        fs::rename(&root, &pending).unwrap();
        assert_eq!(inspect(&root, &pending).unwrap().0, State::Interrupted);
        assert!(create(&root, &pending, b"new-password").is_err());
        assert!(resume(&root, &pending, b"wrong-password").is_err());
        assert!(!root.exists());
        assert!(pending.exists());
        resume(&root, &pending, b"original-password").unwrap();
        assert_eq!(inspect(&root, &pending).unwrap().0, State::Locked);
        local_install::private_dir(&pending).unwrap();
        assert!(inspect(&root, &pending).is_err());
    }
    #[test]
    fn malformed_partial_and_identity_mismatch_fail_closed() {
        let (_dir, root, pending) = fixture();
        local_install::private_dir(&pending).unwrap();
        fs::write(pending.join("evidence"), b"preserve").unwrap();
        assert!(resume(&root, &pending, b"password").is_err());
        assert!(create(&root, &pending, b"password").is_err());
        assert_eq!(fs::read(pending.join("evidence")).unwrap(), b"preserve");
        let (_second, root, pending) = fixture();
        let created = create(&root, &pending, b"original-password").unwrap();
        let marker = fs::read(root.join(MARKER)).unwrap();
        fs::write(root.join(MARKER), b"{}").unwrap();
        assert!(inspect(&root, &pending).is_err());
        assert!(create(&root, &pending, b"replacement-password").is_err());
        fs::write(root.join(MARKER), &marker).unwrap();
        fs::write(
            root.join("VAULT/identity/vault.id"),
            uuid::Uuid::new_v4().to_string(),
        )
        .unwrap();
        assert!(open(&root).is_err());
        fs::write(root.join("VAULT/identity/vault.id"), &created.vault_id).unwrap();
        fs::write(root.join("VAULT/header/header_a.json"), b"malformed header").unwrap();
        assert!(open(&root).is_err());
        assert!(create(&root, &pending, b"replacement-password").is_err());
        assert_eq!(
            fs::read(root.join("VAULT/header/header_a.json")).unwrap(),
            b"malformed header"
        );
    }
}
