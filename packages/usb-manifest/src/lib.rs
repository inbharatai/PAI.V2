//! Strict validation for the removable UnoOne Pocket AI package.
//!
//! The same validator is used by UnoOne Dock, Start UnoOne, and UnoOne Power so
//! no launcher can accidentally apply weaker trust rules than the application.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

pub const PRODUCT_ID: &str = "com.inbharatai.unoone.pocket-ai";
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;
pub const MANIFEST_FILE: &str = "manifest.json";
pub const VERSION_FILE: &str = "VERSION";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocketManifest {
    pub product_id: String,
    pub schema_version: u32,
    pub pai_version: String,
    pub vault: VaultIdentity,
    pub platforms: Platforms,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultIdentity {
    pub id_path: String,
    #[serde(default)]
    pub expected_id: Option<String>,
    #[serde(default)]
    pub id_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Platforms {
    pub windows: WindowsPackage,
    #[serde(default)]
    pub mobile: Option<MobilePackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobilePackage {
    pub architectures: Vec<String>,
    #[serde(default)]
    pub models: Vec<AssetSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowsPackage {
    pub architectures: Vec<String>,
    pub desktop: AssetSpec,
    #[serde(default)]
    pub dock: Option<AssetSpec>,
    #[serde(default)]
    pub starter: Option<AssetSpec>,
    #[serde(default)]
    pub runtimes: Vec<AssetSpec>,
    #[serde(default)]
    pub models: Vec<AssetSpec>,
    #[serde(default)]
    pub voice: Vec<AssetSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetSpec {
    pub id: String,
    pub kind: AssetKind,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    #[serde(default = "default_required")]
    pub required: bool,
    #[serde(default)]
    pub architecture: Option<String>,
}

fn default_required() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AssetKind {
    DesktopExecutable,
    DockExecutable,
    StarterExecutable,
    RuntimeExecutable,
    RuntimeLibrary,
    Model,
    Mmproj,
    WhisperModel,
    PiperModel,
    MobileModel,
    VoiceRuntime,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationScope {
    /// Validate identity, version, desktop executable, all required runtimes,
    /// models, and voice assets. Dock must use this before launch.
    DesktopLaunch,
    /// Validate identity, version, and the starter itself. Used only while
    /// generating/staging a package before the desktop assets are populated.
    PackageIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationFailure {
    pub code: ValidationFailureCode,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidationFailureCode {
    RootMissing,
    ManifestMissing,
    ManifestUnreadable,
    ManifestInvalidJson,
    ProductMismatch,
    SchemaUnsupported,
    VersionMissing,
    VersionMismatch,
    PlatformIncompatible,
    ArchitectureIncompatible,
    VaultIdMissing,
    VaultIdMismatch,
    InvalidRelativePath,
    PathEscape,
    ReparsePointRejected,
    AssetMissing,
    AssetSizeMismatch,
    AssetHashMismatch,
    AssetKindMismatch,
    DuplicateAssetPath,
    InvalidHash,
    RequiredAssetsMissing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedPackage {
    pub root: PathBuf,
    pub manifest: PocketManifest,
    pub vault_id: String,
    pub desktop_executable: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub failures: Vec<ValidationFailure>,
    pub package: Option<ValidatedPackage>,
}

impl ValidationReport {
    fn fail(code: ValidationFailureCode, path: Option<String>, message: impl Into<String>) -> Self {
        Self {
            valid: false,
            failures: vec![ValidationFailure {
                code,
                path,
                message: message.into(),
            }],
            package: None,
        }
    }
}

pub fn validate_package(root: &Path, scope: ValidationScope) -> ValidationReport {
    if !root.is_dir() {
        return ValidationReport::fail(
            ValidationFailureCode::RootMissing,
            Some(root.display().to_string()),
            "Pocket AI root does not exist or is not a directory",
        );
    }

    let root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) => {
            return ValidationReport::fail(
                ValidationFailureCode::RootMissing,
                Some(root.display().to_string()),
                format!("Pocket AI root cannot be resolved: {error}"),
            )
        }
    };

    let manifest_path = root.join(MANIFEST_FILE);
    if !manifest_path.is_file() {
        return ValidationReport::fail(
            ValidationFailureCode::ManifestMissing,
            Some(MANIFEST_FILE.to_string()),
            "manifest.json is missing",
        );
    }

    let manifest_bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ValidationReport::fail(
                ValidationFailureCode::ManifestUnreadable,
                Some(MANIFEST_FILE.to_string()),
                format!("manifest.json cannot be read: {error}"),
            )
        }
    };
    let manifest: PocketManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            return ValidationReport::fail(
                ValidationFailureCode::ManifestInvalidJson,
                Some(MANIFEST_FILE.to_string()),
                format!("manifest.json does not match schema v2: {error}"),
            )
        }
    };

    let mut failures = Vec::new();
    if manifest.product_id != PRODUCT_ID {
        failures.push(failure(
            ValidationFailureCode::ProductMismatch,
            Some(MANIFEST_FILE),
            format!(
                "Expected product_id {PRODUCT_ID}, got {}",
                manifest.product_id
            ),
        ));
    }
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        failures.push(failure(
            ValidationFailureCode::SchemaUnsupported,
            Some(MANIFEST_FILE),
            format!(
                "Expected schema version {MANIFEST_SCHEMA_VERSION}, got {}",
                manifest.schema_version
            ),
        ));
    }

    let version_path = root.join(VERSION_FILE);
    match fs::read_to_string(&version_path) {
        Ok(version) if version.trim() == manifest.pai_version => {}
        Ok(version) => failures.push(failure(
            ValidationFailureCode::VersionMismatch,
            Some(VERSION_FILE),
            format!(
                "VERSION contains {:?}, manifest declares {:?}",
                version.trim(),
                manifest.pai_version
            ),
        )),
        Err(error) => failures.push(failure(
            ValidationFailureCode::VersionMissing,
            Some(VERSION_FILE),
            format!("VERSION cannot be read: {error}"),
        )),
    }

    if !manifest
        .platforms
        .windows
        .architectures
        .iter()
        .any(|arch| arch.eq_ignore_ascii_case(current_architecture()))
    {
        failures.push(failure(
            ValidationFailureCode::ArchitectureIncompatible,
            Some(MANIFEST_FILE),
            format!(
                "Package supports {:?}, host architecture is {}",
                manifest.platforms.windows.architectures,
                current_architecture()
            ),
        ));
    }

    let vault_id_path = match resolve_bounded_path(&root, &manifest.vault.id_path) {
        Ok(path) => path,
        Err(problem) => {
            failures.push(problem);
            PathBuf::new()
        }
    };
    let mut vault_id = String::new();
    if !vault_id_path.as_os_str().is_empty() {
        match fs::read_to_string(&vault_id_path) {
            Ok(value) if !value.trim().is_empty() => {
                vault_id = value.trim().to_string();
                if let Some(expected) = manifest.vault.expected_id.as_deref() {
                    if expected != vault_id {
                        failures.push(failure(
                            ValidationFailureCode::VaultIdMismatch,
                            Some(&manifest.vault.id_path),
                            "vault.id does not match the manifest identity",
                        ));
                    }
                }
                if let Some(expected_hash) = manifest.vault.id_sha256.as_deref() {
                    validate_hash_text(
                        expected_hash,
                        &vault_id_path,
                        &manifest.vault.id_path,
                        &mut failures,
                    );
                }
            }
            Ok(_) => failures.push(failure(
                ValidationFailureCode::VaultIdMissing,
                Some(&manifest.vault.id_path),
                "vault.id is empty",
            )),
            Err(error) => failures.push(failure(
                ValidationFailureCode::VaultIdMissing,
                Some(&manifest.vault.id_path),
                format!("vault.id cannot be read: {error}"),
            )),
        }
    }

    let windows = &manifest.platforms.windows;
    validate_asset_kind(
        &windows.desktop,
        &[AssetKind::DesktopExecutable],
        &mut failures,
    );
    if let Some(dock) = windows.dock.as_ref() {
        validate_asset_kind(dock, &[AssetKind::DockExecutable], &mut failures);
    }
    if let Some(starter) = windows.starter.as_ref() {
        validate_asset_kind(starter, &[AssetKind::StarterExecutable], &mut failures);
    }
    for runtime in &windows.runtimes {
        validate_asset_kind(
            runtime,
            &[AssetKind::RuntimeExecutable, AssetKind::RuntimeLibrary],
            &mut failures,
        );
    }
    for model in &windows.models {
        validate_asset_kind(model, &[AssetKind::Model, AssetKind::Mmproj], &mut failures);
    }
    for voice in &windows.voice {
        validate_asset_kind(
            voice,
            &[
                AssetKind::VoiceRuntime,
                AssetKind::RuntimeLibrary,
                AssetKind::WhisperModel,
                AssetKind::PiperModel,
            ],
            &mut failures,
        );
    }

    let mut assets: Vec<&AssetSpec> = vec![&windows.desktop];
    if scope == ValidationScope::DesktopLaunch {
        assets.extend(windows.runtimes.iter().filter(|asset| asset.required));
        assets.extend(windows.models.iter().filter(|asset| asset.required));
        assets.extend(windows.voice.iter().filter(|asset| asset.required));
    }
    if let Some(starter) = windows.starter.as_ref() {
        assets.push(starter);
    }
    if let Some(dock) = windows.dock.as_ref() {
        assets.push(dock);
    }

    let missing_required_launch_assets = !windows
        .runtimes
        .iter()
        .any(|asset| asset.required && matches!(asset.kind, AssetKind::RuntimeExecutable))
        || !windows
            .models
            .iter()
            .any(|asset| asset.required && matches!(asset.kind, AssetKind::Model));
    if scope == ValidationScope::DesktopLaunch && missing_required_launch_assets {
        failures.push(failure(
            ValidationFailureCode::RequiredAssetsMissing,
            Some(MANIFEST_FILE),
            "At least one required Windows runtime executable and model must be declared",
        ));
    }

    let mut unique_paths = HashSet::new();
    for asset in &assets {
        let normalized = asset.path.replace('\\', "/").to_ascii_lowercase();
        if !unique_paths.insert(normalized) {
            failures.push(failure(
                ValidationFailureCode::DuplicateAssetPath,
                Some(&asset.path),
                "Each declared Windows asset must use a unique path",
            ));
        }
        if asset.required {
            if let Some(architecture) = asset.architecture.as_deref() {
                if !architecture.eq_ignore_ascii_case(current_architecture()) {
                    failures.push(failure(
                        ValidationFailureCode::ArchitectureIncompatible,
                        Some(&asset.path),
                        format!(
                            "Required asset {} targets {}, host architecture is {}",
                            asset.id,
                            architecture,
                            current_architecture()
                        ),
                    ));
                }
            }
        }
    }

    for asset in assets {
        validate_asset(&root, asset, &mut failures);
    }

    if !failures.is_empty() {
        return ValidationReport {
            valid: false,
            failures,
            package: None,
        };
    }

    let desktop_executable = resolve_bounded_path(&root, &windows.desktop.path)
        .expect("desktop path was validated above");
    ValidationReport {
        valid: true,
        failures: Vec::new(),
        package: Some(ValidatedPackage {
            root,
            manifest,
            vault_id,
            desktop_executable,
        }),
    }
}

fn validate_asset_kind(
    asset: &AssetSpec,
    allowed: &[AssetKind],
    failures: &mut Vec<ValidationFailure>,
) {
    if !allowed.contains(&asset.kind) {
        failures.push(failure(
            ValidationFailureCode::AssetKindMismatch,
            Some(&asset.path),
            format!(
                "Asset {} has kind {:?}; expected one of {:?}",
                asset.id, asset.kind, allowed
            ),
        ));
    }
}

fn validate_asset(root: &Path, asset: &AssetSpec, failures: &mut Vec<ValidationFailure>) {
    let path = match resolve_bounded_path(root, &asset.path) {
        Ok(path) => path,
        Err(problem) => {
            failures.push(problem);
            return;
        }
    };
    if !path.is_file() {
        if asset.required {
            failures.push(failure(
                ValidationFailureCode::AssetMissing,
                Some(&asset.path),
                format!("Required {} asset {} is missing", asset.id, asset.path),
            ));
        }
        return;
    }
    match fs::metadata(&path) {
        Ok(metadata) if metadata.len() != asset.size_bytes => failures.push(failure(
            ValidationFailureCode::AssetSizeMismatch,
            Some(&asset.path),
            format!(
                "{} expected {} bytes, found {}",
                asset.id,
                asset.size_bytes,
                metadata.len()
            ),
        )),
        Err(error) => failures.push(failure(
            ValidationFailureCode::AssetMissing,
            Some(&asset.path),
            format!("{} metadata cannot be read: {error}", asset.id),
        )),
        _ => {}
    }
    validate_hash_text(&asset.sha256, &path, &asset.path, failures);
}

fn validate_hash_text(
    expected: &str,
    path: &Path,
    relative_path: &str,
    failures: &mut Vec<ValidationFailure>,
) {
    if expected.len() != 64 || hex::decode(expected).is_err() {
        failures.push(failure(
            ValidationFailureCode::InvalidHash,
            Some(relative_path),
            "Expected SHA-256 must contain exactly 64 hexadecimal characters",
        ));
        return;
    }
    match sha256_file(path) {
        Ok(actual) if !actual.eq_ignore_ascii_case(expected) => failures.push(failure(
            ValidationFailureCode::AssetHashMismatch,
            Some(relative_path),
            format!("SHA-256 mismatch: expected {expected}, found {actual}"),
        )),
        Err(error) => failures.push(failure(
            ValidationFailureCode::AssetMissing,
            Some(relative_path),
            format!("Asset cannot be hashed: {error}"),
        )),
        _ => {}
    }
}

fn resolve_bounded_path(root: &Path, relative: &str) -> Result<PathBuf, ValidationFailure> {
    let relative_path = Path::new(relative);
    if relative.trim().is_empty()
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(failure(
            ValidationFailureCode::InvalidRelativePath,
            Some(relative),
            "Manifest paths must be non-empty, relative, and contain no traversal",
        ));
    }

    let mut candidate = root.to_path_buf();
    for component in relative_path.components() {
        candidate.push(component.as_os_str());
        if let Ok(metadata) = fs::symlink_metadata(&candidate) {
            if metadata.file_type().is_symlink() || is_windows_reparse_point(&metadata) {
                return Err(failure(
                    ValidationFailureCode::ReparsePointRejected,
                    Some(relative),
                    "Symlink, junction, or reparse-point paths are not trusted",
                ));
            }
        }
    }

    if candidate.exists() {
        let canonical = fs::canonicalize(&candidate).map_err(|error| {
            failure(
                ValidationFailureCode::PathEscape,
                Some(relative),
                format!("Asset path cannot be resolved: {error}"),
            )
        })?;
        if !canonical.starts_with(root) {
            return Err(failure(
                ValidationFailureCode::PathEscape,
                Some(relative),
                "Resolved asset path escapes the Pocket AI root",
            ));
        }
        Ok(canonical)
    } else {
        Ok(candidate)
    }
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    // A 1 MiB stack array overflows the default stack of optimized Windows GUI
    // binaries before the first asset can be verified. Keep the large streaming
    // buffer on the heap so Starter, Dock, and Power share the same safe path.
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

fn current_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
}

fn failure(
    code: ValidationFailureCode,
    path: Option<impl AsRef<str>>,
    message: impl Into<String>,
) -> ValidationFailure {
    ValidationFailure {
        code,
        path: path.map(|value| value.as_ref().to_string()),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn sha(data: &[u8]) -> String {
        hex::encode_upper(Sha256::digest(data))
    }

    fn fixture() -> (TempDir, PocketManifest) {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("VAULT/identity")).unwrap();
        fs::create_dir_all(temp.path().join("APPS/WINDOWS")).unwrap();
        fs::create_dir_all(temp.path().join("RUNTIMES/WINDOWS/CPU")).unwrap();
        fs::create_dir_all(temp.path().join("MODELS/DESKTOP")).unwrap();
        fs::write(temp.path().join(VERSION_FILE), "0.6.0-alpha\n").unwrap();
        fs::write(temp.path().join("VAULT/identity/vault.id"), "test-vault\n").unwrap();
        fs::write(temp.path().join("APPS/WINDOWS/UnoOnePower.exe"), b"power").unwrap();
        fs::write(
            temp.path().join("RUNTIMES/WINDOWS/CPU/llama-server.exe"),
            b"runtime",
        )
        .unwrap();
        fs::write(temp.path().join("MODELS/DESKTOP/model.gguf"), b"model").unwrap();
        let manifest = PocketManifest {
            product_id: PRODUCT_ID.to_string(),
            schema_version: MANIFEST_SCHEMA_VERSION,
            pai_version: "0.6.0-alpha".to_string(),
            vault: VaultIdentity {
                id_path: "VAULT/identity/vault.id".to_string(),
                expected_id: Some("test-vault".to_string()),
                id_sha256: None,
            },
            platforms: Platforms {
                windows: WindowsPackage {
                    architectures: vec![current_architecture().to_string()],
                    desktop: AssetSpec {
                        id: "unoone-power".to_string(),
                        kind: AssetKind::DesktopExecutable,
                        path: "APPS/WINDOWS/UnoOnePower.exe".to_string(),
                        size_bytes: 5,
                        sha256: sha(b"power"),
                        required: true,
                        architecture: Some(current_architecture().to_string()),
                    },
                    dock: None,
                    starter: None,
                    runtimes: vec![AssetSpec {
                        id: "llama-cpu".to_string(),
                        kind: AssetKind::RuntimeExecutable,
                        path: "RUNTIMES/WINDOWS/CPU/llama-server.exe".to_string(),
                        size_bytes: 7,
                        sha256: sha(b"runtime"),
                        required: true,
                        architecture: Some(current_architecture().to_string()),
                    }],
                    models: vec![AssetSpec {
                        id: "gemma".to_string(),
                        kind: AssetKind::Model,
                        path: "MODELS/DESKTOP/model.gguf".to_string(),
                        size_bytes: 5,
                        sha256: sha(b"model"),
                        required: true,
                        architecture: None,
                    }],
                    voice: vec![],
                },
                mobile: None,
            },
        };
        (temp, manifest)
    }

    fn write_manifest(root: &Path, manifest: &PocketManifest) {
        let mut file = File::create(root.join(MANIFEST_FILE)).unwrap();
        file.write_all(serde_json::to_string_pretty(manifest).unwrap().as_bytes())
            .unwrap();
    }

    #[test]
    fn accepts_complete_package() {
        let (temp, manifest) = fixture();
        write_manifest(temp.path(), &manifest);
        let report = validate_package(temp.path(), ValidationScope::DesktopLaunch);
        assert!(report.valid, "{:?}", report.failures);
        assert_eq!(report.package.unwrap().vault_id, "test-vault");
    }

    #[test]
    fn rejects_traversal_before_reading_asset() {
        let (temp, mut manifest) = fixture();
        manifest.platforms.windows.desktop.path = "../evil.exe".to_string();
        write_manifest(temp.path(), &manifest);
        let report = validate_package(temp.path(), ValidationScope::DesktopLaunch);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == ValidationFailureCode::InvalidRelativePath));
    }

    #[test]
    fn rejects_tampered_executable() {
        let (temp, manifest) = fixture();
        write_manifest(temp.path(), &manifest);
        fs::write(temp.path().join("APPS/WINDOWS/UnoOnePower.exe"), b"changed").unwrap();
        let report = validate_package(temp.path(), ValidationScope::DesktopLaunch);
        assert!(report.failures.iter().any(|failure| {
            matches!(
                failure.code,
                ValidationFailureCode::AssetSizeMismatch | ValidationFailureCode::AssetHashMismatch
            )
        }));
    }

    #[test]
    fn rejects_spoofed_desktop_kind() {
        let (temp, mut manifest) = fixture();
        manifest.platforms.windows.desktop.kind = AssetKind::RuntimeLibrary;
        write_manifest(temp.path(), &manifest);
        let report = validate_package(temp.path(), ValidationScope::DesktopLaunch);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == ValidationFailureCode::AssetKindMismatch));
    }

    #[test]
    fn rejects_duplicate_required_asset_path() {
        let (temp, mut manifest) = fixture();
        manifest.platforms.windows.models[0].path =
            manifest.platforms.windows.runtimes[0].path.clone();
        manifest.platforms.windows.models[0].size_bytes =
            manifest.platforms.windows.runtimes[0].size_bytes;
        manifest.platforms.windows.models[0].sha256 =
            manifest.platforms.windows.runtimes[0].sha256.clone();
        write_manifest(temp.path(), &manifest);
        let report = validate_package(temp.path(), ValidationScope::DesktopLaunch);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == ValidationFailureCode::DuplicateAssetPath));
    }

    #[test]
    fn rejects_incompatible_required_asset_architecture() {
        let (temp, mut manifest) = fixture();
        manifest.platforms.windows.desktop.architecture =
            Some("incompatible-test-arch".to_string());
        write_manifest(temp.path(), &manifest);
        let report = validate_package(temp.path(), ValidationScope::DesktopLaunch);
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.code == ValidationFailureCode::ArchitectureIncompatible));
    }
}
