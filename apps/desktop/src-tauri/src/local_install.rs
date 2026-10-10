//! Production local storage policy. No Tauri, media discovery or model authority.
//! Paths are supplied by Tauri's OS app-local-data resolver, never by the webview.
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const ROOT_NAME: &str = "local-install";
pub const PENDING_NAME: &str = "local-install.pending";

pub fn present(path: &Path) -> bool {
    // includes dangling symlinks (exists() would incorrectly call those absent).
    fs::symlink_metadata(path).is_ok()
}

/// Reject traversal, symlinks and Windows reparse points at every existing
/// ancestor. Missing suffixes are permitted for first creation, never fabricated.
pub fn checked_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Local storage requires an absolute path without traversal".into());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) => reject_link(&meta)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot inspect local storage: {e}")),
        }
    }
    Ok(())
}

fn reject_link(meta: &fs::Metadata) -> Result<(), String> {
    if meta.file_type().is_symlink() {
        return Err("Local storage cannot contain symbolic links".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err("Local storage cannot contain junctions or reparse points".into());
        }
    }
    Ok(())
}

pub fn roots(app_data: &Path) -> Result<(PathBuf, PathBuf), String> {
    checked_path(app_data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        let anchor = app_data
            .ancestors()
            .find(|p| present(p))
            .ok_or("Missing app-data ancestor")?;
        let meta = fs::metadata(anchor).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.uid() != unsafe { geteuid() } || meta.mode() & 0o022 != 0 {
            return Err(
                "App-data parent must be owned by this user and not writable by other users".into(),
            );
        }
    }
    let root = app_data.join(ROOT_NAME);
    let pending = app_data.join(PENDING_NAME);
    checked_path(&root)?;
    checked_path(&pending)?;
    Ok((root, pending))
}

/// Create only missing directories; never chmod, delete or replace an existing
/// directory. An unsafe pre-existing local root is a repair-required error.
pub fn private_dir(path: &Path) -> Result<(), String> {
    checked_path(path)?;
    if present(path) {
        return private_owner(path);
    }
    let parent = path.parent().ok_or("Missing local storage parent")?;
    if !present(parent) {
        private_dir(parent)?;
    }
    claim_private_dir(path)
}

/// Exclusive mkdir: even a concurrent first-creation attempt cannot reuse it.
/// The caller must prepare its parent first. Failure leaves all files intact.
pub fn claim_private_dir(path: &Path) -> Result<(), String> {
    checked_path(path)?;
    #[allow(unused_mut)] // mode() is Unix-only
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|e| format!("Cannot create private directory: {e}"))?;
    #[cfg(windows)]
    windows_acl(path, true)?;
    private_owner(path)
}

pub fn private_owner(path: &Path) -> Result<(), String> {
    checked_path(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err("Local storage root is not a directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        if meta.uid() != unsafe { geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(
                "Local storage must be owned by this user with owner-only permissions (0700)"
                    .into(),
            );
        }
    }
    #[cfg(windows)]
    windows_acl(path, false)?;
    Ok(())
}

#[cfg(windows)]
fn windows_acl(path: &Path, create: bool) -> Result<(), String> {
    // Fixed script; path passed as environment data, never interpolated as code.
    // Use the current SID, not a localized username. Fail closed if ACL setup or
    // verification is unavailable. Must be physically qualified on Windows.
    let script = r#"$ErrorActionPreference='Stop'; $p=$env:UNOONE_PRIVATE_DIRECTORY; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; if($env:UNOONE_CREATE_ACL -eq '1'){ $acl=New-Object System.Security.AccessControl.DirectorySecurity; $acl.SetOwner($sid); $acl.SetAccessRuleProtection($true,$false); $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); $acl.AddAccessRule($rule); Set-Acl -LiteralPath $p -AclObject $acl }; $a=Get-Acl -LiteralPath $p; if($a.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value){throw 'owner mismatch'}; $ok=$false; foreach($r in $a.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])){ if($r.AccessControlType -eq 'Allow'){if($r.IdentityReference.Value -ne $sid.Value){throw 'non-owner access'}; $ok=$true} }; if(!$ok){throw 'missing owner access'}"#;
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("UNOONE_PRIVATE_DIRECTORY", path)
        .env("UNOONE_CREATE_ACL", if create { "1" } else { "0" })
        .output()
        .map_err(|_| "Cannot verify private local storage ACL".to_string())?;
    if !status.status.success() {
        return Err("Private local storage ACL validation failed".into());
    }
    Ok(())
}

/// Bounded metadata walk: rejects links, reparse points, devices, hard links,
/// and path escapes before opening vault-core or copying an encrypted backup.
pub fn checked_tree(root: &Path) -> Result<Vec<PathBuf>, String> {
    private_owner(root)?;
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut files = Vec::new();
    let mut count = 0;
    while let Some((dir, depth)) = pending.pop() {
        if depth > 32 {
            return Err("Local vault directory depth exceeds limit".into());
        }
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            reject_link(&meta)?;
            count += 1;
            if count > 100_000 {
                return Err("Local vault entry limit exceeded".into());
            }
            if meta.is_dir() {
                pending.push((path, depth + 1));
            } else if meta.is_file() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.nlink() != 1 {
                        return Err("Hard-linked local vault files are not allowed".into());
                    }
                }
                files.push(path);
            } else {
                return Err("Special files are not allowed in a local vault".into());
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "local-root-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        private_dir(&p).unwrap();
        p
    }
    #[test]
    fn requires_absolute_and_rejects_traversal() {
        assert!(checked_path(Path::new("relative")).is_err());
        assert!(checked_path(&std::env::temp_dir().join("a/../b")).is_err());
    }
    #[test]
    fn derives_distinct_roots_without_writing() {
        let p = temp();
        let (r, s) = roots(&p).unwrap();
        assert_ne!(r, s);
        assert!(!present(&r));
        assert!(!present(&s));
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn exclusive_creation_refuses_even_an_empty_existing_directory() {
        let p = temp();
        assert!(claim_private_dir(&p).is_err());
        assert!(p.is_dir());
        fs::remove_dir_all(p).unwrap();
    }
    #[test]
    fn refuses_existing_file_without_deletion() {
        let p = temp();
        let f = p.join("file");
        fs::write(&f, b"keep").unwrap();
        assert!(private_dir(&f).is_err());
        assert_eq!(fs::read(f).unwrap(), b"keep");
        fs::remove_dir_all(p).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlink_and_dangling_link_and_hardlink() {
        use std::os::unix::fs::symlink;
        let p = temp();
        symlink(p.join("missing"), p.join("link")).unwrap();
        assert!(present(&p.join("link")));
        assert!(checked_path(&p.join("link/new")).is_err());
        assert!(checked_tree(&p).is_err());
        fs::remove_file(p.join("link")).unwrap();
        fs::write(p.join("a"), b"keep").unwrap();
        fs::hard_link(p.join("a"), p.join("b")).unwrap();
        assert!(checked_tree(&p).is_err());
        fs::remove_dir_all(p).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn rejects_shared_writable_app_data_parent() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o770)).unwrap();
        assert!(roots(&p.join("future-app-dir")).is_err());
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(roots(&p.join("future-app-dir")).is_ok());
        fs::remove_dir_all(p).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn refuses_broad_existing_permissions_without_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&p).is_err());
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(p).unwrap();
    }
}
