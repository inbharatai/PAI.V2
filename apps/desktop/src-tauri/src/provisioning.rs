//! Bounded Power adapter phase. No release key/catalog was supplied: shipping
//! configuration is deliberately empty, never populated from renderer or web JSON.
//! Existing distribution manifests are integrity inventories, not qualifications.
use base64::{engine::general_purpose::STANDARD, Engine};
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use unoone_model_admission::*;

pub const BLOCKED: &str = "Local model setup is blocked: the publisher has not configured a trusted signing key, approved compatibility catalog and matching qualification records. No model will be downloaded or loaded. Existing models and vault data are unchanged.";

/// Reviewed app-owned configuration. Exact payload digest pins bind domain,
/// approved release lineage and rollback floor; key rotation/revocation requires
/// an app configuration update. Signature is over EXACT payload bytes, no JSON
/// reserialization or undocumented prefix. Pins do not replace the signature.
pub struct TrustedPayload {
    pub domain: SignedDomain,
    pub key_id: String,
    pub public_key: [u8; 32],
    pub payload_sha256: [u8; 32],
}
pub struct RingVerifier {
    approved: Vec<TrustedPayload>,
}
impl RingVerifier {
    pub fn configured(approved: Vec<TrustedPayload>) -> Self {
        Self { approved }
    }
    pub fn shipping() -> Self {
        Self::configured(vec![])
    }
}
impl SignatureVerifier for RingVerifier {
    fn verify(
        &self,
        domain: SignedDomain,
        payload: &[u8],
        a: &Attestation,
    ) -> Result<(), ContractError> {
        if a.algorithm != "Ed25519" || payload.len() > MAX_WIRE_BYTES || a.signature.len() > 128 {
            return Err(ContractError::VerificationFailed);
        }
        let digest: [u8; 32] = Sha256::digest(payload).into();
        let trust = self
            .approved
            .iter()
            .find(|k| k.domain == domain && k.key_id == a.key_id && k.payload_sha256 == digest)
            .ok_or(ContractError::VerificationFailed)?;
        let signature = STANDARD
            .decode(&a.signature)
            .map_err(|_| ContractError::VerificationFailed)?;
        UnparsedPublicKey::new(&ED25519, trust.public_key)
            .verify(payload, &signature)
            .map_err(|_| ContractError::VerificationFailed)
    }
}

#[derive(Serialize)]
pub struct SetupAssessment {
    pub schema_version: u32,
    pub catalog_state: &'static str,
    pub reason: &'static str,
    pub eligible_now: bool,
    pub assets_ready: bool,
    pub device: DeviceProbe,
    pub decisions: Vec<Decision>,
    pub stages: [&'static str; 5],
    /// Declared model files already present in the selected root, each with the
    /// native three-state label (QUALIFIED / WORKS_HERE / UNKNOWN). Display and
    /// explanation only — native select/start recompute the decision.
    pub local_models: Vec<LocalModelReport>,
}
pub async fn assessment(root: &Path) -> SetupAssessment {
    let device = crate::provisioning_probe::probe(root).await;
    let total = device.total_ram_bytes.measured().copied();
    let request = default_local_request(total);
    let signed = load_signed_records(root);
    let verifier = RingVerifier::shipping();
    let now = crate::provisioning_probe::now_ms();
    let local_models = local_inventory(root)
        .iter()
        .map(|file| decide_local(root, file, &device, &request, &signed, &verifier, now))
        .collect::<Vec<_>>();
    let decisions = local_models
        .iter()
        .filter_map(|m| m.decision.clone())
        .collect();
    SetupAssessment {
        schema_version: 1,
        catalog_state: "UNCONFIGURED",
        reason: BLOCKED,
        eligible_now: false,
        assets_ready: false,
        device,
        decisions,
        local_models,
        stages: [
            "DEVICE_CHECK",
            "BLOCKED_CATALOG",
            "AWAITING_LOCAL_POLICY",
            "NOT_DOWNLOADED",
            "NOT_SMOKE_TESTED",
        ],
    }
}
/// Shipping ACQUISITION boundary: catalog suggestion and any download stay
/// fail-closed until a trusted signing key, approved catalog and qualification
/// records exist. Never unlock this by file presence, renderer bool, or an
/// unsigned manifest. Loading an ALREADY PRESENT, declared, hash-verified file
/// on this machine is a separate decision — see [`decide_local`] below — and is
/// never labelled as qualification.
pub fn require_shipping_admission() -> Result<(), String> {
    Err(BLOCKED.into())
}

/// Adapter reuse once reviewed configuration exists: same core decision at every
/// boundary, never accept a renderer-supplied decision or measured probe.
pub fn decision(
    c: &VerifiedCandidate,
    q: &[ValidatedQualification],
    p: &DeviceProbe,
    r: &AdmissionRequest,
) -> Decision {
    evaluate(c, p, r, q, None, crate::provisioning_probe::now_ms())
}

/// Native-only local approval store. No command accepts a policy JSON/approved
/// bool. The host must create the policy from signed catalog + a locally reviewed
/// scope, authenticate the main window/unlocked vault, then store it. Fresh reads
/// are mandatory on every transfer boundary. Not synced, not in the vault format.
pub struct LocalConsentStore {
    directory: PathBuf,
}
impl LocalConsentStore {
    pub fn open(selected_install_root: &Path) -> Result<Self, String> {
        crate::local_install::private_owner(selected_install_root)?;
        let directory = selected_install_root.join("model-provisioning-policy");
        crate::local_install::private_dir(&directory)?;
        Ok(Self { directory })
    }
    pub(crate) fn approve_from_native(
        &self,
        policy: &StandingDownloadPolicy,
    ) -> Result<(), String> {
        let path = self.directory.join("current.json");
        crate::local_install::checked_path(&path)?;
        // Revoke first; interrupted policy change must never leave an older grant live.
        self.revoke()?;
        let bytes = serde_json::to_vec(policy).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_WIRE_BYTES {
            return Err("Policy exceeds bound".into());
        }
        let pending = self
            .directory
            .join(format!("pending-{}.json", uuid::Uuid::new_v4()));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(|e| e.to_string())?;
        f.write_all(&bytes)
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&pending, &path).map_err(|e| e.to_string())?;
        sync_directory(&self.directory)
    }
    pub fn revoke(&self) -> Result<(), String> {
        let path = self.directory.join("current.json");
        crate::local_install::checked_path(&path)?;
        match fs::remove_file(path) {
            Ok(()) => sync_directory(&self.directory),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    fn current(&self) -> Result<StandingDownloadPolicy, ContractError> {
        crate::local_install::private_owner(&self.directory)
            .map_err(|_| ContractError::VerificationFailed)?;
        let path = self.directory.join("current.json");
        safe_regular_file(&path).map_err(|_| ContractError::VerificationFailed)?;
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|_| ContractError::VerificationFailed)?
            .take((MAX_WIRE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ContractError::VerificationFailed)?;
        decode(&bytes)
    }
    pub fn grant(&self) -> Result<NativePolicyGrant, ContractError> {
        NativePolicyGrant::from_local_store(self.current()?, self)
    }
}
impl StandingPolicyAuthority for LocalConsentStore {
    fn validate_local_approval(
        &self,
        policy: &StandingDownloadPolicy,
    ) -> Result<(), ContractError> {
        if &self.current()? != policy {
            return Err(ContractError::VerificationFailed);
        }
        let now = crate::provisioning_probe::now_ms();
        if now < policy.not_before_ms || now >= policy.expires_at_ms {
            return Err(ContractError::Expired);
        }
        Ok(())
    }
}

pub(crate) fn safe_regular_file(path: &Path) -> Result<(), String> {
    crate::local_install::checked_path(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err("Hard links are not allowed".into());
        }
    }
    Ok(())
}
pub(crate) fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    // Windows directory flush semantics require physical qualification. File
    // sync + same-volume rename is used; no sudden-power-loss durability claim.
    let _ = path;
    Ok(())
}

// ---------------------------------------------------------------------------
// Local route: ALREADY PRESENT, declared, hash-verified model files on this
// machine. One decision function for selection, server start and the Model /
// Hardware views. Nothing here downloads, promotes a manifest to a catalog, or
// mints qualification: a successful native load + inference smoke on this
// device is recorded as "works here (not yet qualified)", never "qualified".
// ---------------------------------------------------------------------------

/// Runtime reserve added on top of weights + projector + KV when checking the
/// CURRENTLY AVAILABLE RAM (the legacy total-RAM rule in desktop_model_policy
/// uses the same 1 GiB constant). Estimate, not a measurement of peak RSS.
pub const LOCAL_AVAILABLE_RESERVE_BYTES: u64 = 1 << 30;
/// KV estimate when the GGUF header does not declare enough shape metadata —
/// the same assumption the legacy drive lane makes (llama.rs admit_model).
pub const LOCAL_KV_FALLBACK_BYTES: u64 = 1 << 30;
const LOCAL_VERIFICATION_DIR: &str = "model-verification";
const LOCAL_SIGNED_DIR: &str = "model-qualification";
pub const LABEL_QUALIFIED: &str = "Qualified (signed record)";
pub const LABEL_WORKS_HERE: &str = "Works on this machine (not yet qualified)";
pub const LABEL_FITS_UNTESTED: &str =
    "Fits this machine — not yet load-tested here (not yet qualified)";
pub const LABEL_UNKNOWN: &str = "Unknown";

/// A model file DECLARED by a manifest in the selected root (typed Pocket
/// manifest or the legacy `models.desktop` JSON). The declared digest is the
/// identity; the bytes are verified against it here and again before spawn.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LocalModelFile {
    pub id: String,
    pub path: PathBuf,
    pub expected_sha256: Option<String>,
    pub mmproj_path: Option<PathBuf>,
    pub expected_mmproj_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LocalState {
    /// A signed, scope-matching qualification record admitted this exact file.
    Qualified,
    /// Native load + inference smoke succeeded on THIS device for this digest.
    WorksHere,
    /// No evidence beyond presence/estimates (includes refused/unavailable).
    Unknown,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LocalOutcome {
    /// Core `evaluate()` returned RECOMMENDED for a verified signed candidate.
    Recommended,
    /// Load may proceed; labelled "not yet qualified" (or qualified-with-limits).
    AllowedWithLimits,
    /// File/digest/projector/tier/measurement missing — nothing to load.
    Unavailable,
    /// Present but refused: digest mismatch, total or available RAM insufficient,
    /// or a core UNSUPPORTED/not-eligible decision.
    Refused,
}
impl LocalOutcome {
    pub fn allows_load(self) -> bool {
        matches!(self, Self::Recommended | Self::AllowedWithLimits)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LocalRequest {
    pub context_tokens: u32,
    pub cache_type_k: Option<String>,
}
/// Same RAM-stepped ladder as llama::get_model_config (kept in sync by test).
pub fn default_local_request(total_ram_bytes: Option<u64>) -> LocalRequest {
    let gib = total_ram_bytes.map(|b| b >> 30).unwrap_or(0);
    if gib >= 24 {
        LocalRequest {
            context_tokens: 32768,
            cache_type_k: Some("q8_0".into()),
        }
    } else if gib >= 12 {
        LocalRequest {
            context_tokens: 16384,
            cache_type_k: Some("q8_0".into()),
        }
    } else {
        LocalRequest {
            context_tokens: 4096,
            cache_type_k: None,
        }
    }
}

/// This-device inference smoke evidence (bounded completion returned tokens).
/// Measured count/time only; never converted into a tokens-per-second claim.
#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SmokeEvidence {
    pub schema_version: u32,
    pub sha256: String,
    pub generated_tokens: u64,
    pub generation_ms: u64,
    pub content_chars: u64,
    pub captured_at_ms: u64,
    pub os: String,
    pub total_ram_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocalModelReport {
    pub id: String,
    pub path: String,
    pub mmproj_path: Option<String>,
    pub tier: Option<String>,
    pub present: bool,
    pub hash_verified: bool,
    pub weights_bytes: Option<u64>,
    pub projector_bytes: Option<u64>,
    /// ESTIMATED from the GGUF header for `context_tokens`; None = fallback used.
    pub kv_estimate_bytes: Option<u64>,
    pub context_tokens: u32,
    pub total_ram_bytes: Option<u64>,
    pub available_ram_bytes: Option<u64>,
    /// weights + projector + KV + reserve, compared against AVAILABLE RAM.
    pub required_available_bytes: Option<u64>,
    pub state: LocalState,
    pub outcome: LocalOutcome,
    pub label: &'static str,
    pub reasons: Vec<String>,
    /// Core `evaluate()` output whenever a verified signed candidate matched.
    pub decision: Option<Decision>,
    pub smoke: Option<SmokeEvidence>,
}

fn tier_name(t: crate::desktop_model_policy::Tier) -> &'static str {
    match t {
        crate::desktop_model_policy::Tier::E2B => "E2B",
        crate::desktop_model_policy::Tier::E4B => "E4B",
        crate::desktop_model_policy::Tier::TwelveB => "12B",
    }
}

/// Mirror of llama::ModelManager::find_models' manifest discovery (both
/// formats), kept dependency-free so the external harness compiles it. Scanned
/// directories without a declared digest are deliberately NOT inventory.
pub fn local_inventory(root: &Path) -> Vec<LocalModelFile> {
    let Ok(text) = fs::read_to_string(root.join("manifest.json")) else {
        return vec![];
    };
    if text.len() > 4 * MAX_WIRE_BYTES {
        return vec![];
    }
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let mut out = vec![];
    if let Some(assets) = manifest
        .pointer("/platforms/windows/models")
        .and_then(|v| v.as_array())
    {
        let field =
            |a: &serde_json::Value, k: &str| a.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        for asset in assets
            .iter()
            .filter(|a| field(a, "kind").as_deref() == Some("MODEL"))
        {
            let (Some(id), Some(path)) = (field(asset, "id"), field(asset, "path")) else {
                continue;
            };
            let tier = crate::desktop_model_policy::tier(&id);
            let projector = assets.iter().find(|a| {
                field(a, "kind").as_deref() == Some("MMPROJ")
                    && tier.is_some()
                    && crate::desktop_model_policy::tier(&format!(
                        "{} {}",
                        field(a, "id").unwrap_or_default(),
                        field(a, "path").unwrap_or_default()
                    )) == tier
            });
            out.push(LocalModelFile {
                id,
                path: root.join(path),
                expected_sha256: field(asset, "sha256"),
                mmproj_path: projector
                    .and_then(|a| field(a, "path"))
                    .map(|p| root.join(p)),
                expected_mmproj_sha256: projector.and_then(|a| field(a, "sha256")),
            });
        }
        return out;
    }
    if let Some(desktop) = manifest
        .pointer("/models/desktop")
        .and_then(|v| v.as_object())
    {
        for (key, model) in desktop {
            let Some(path) = model.get("path").and_then(|v| v.as_str()) else {
                continue;
            };
            out.push(LocalModelFile {
                id: model
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(key)
                    .to_owned(),
                path: root.join(path),
                expected_sha256: model
                    .get("sha256")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                mmproj_path: model
                    .get("mmproj_path")
                    .and_then(|v| v.as_str())
                    .map(|p| root.join(p)),
                expected_mmproj_sha256: model
                    .get("mmproj_sha256")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            });
        }
    }
    out
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
fn marker_value(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(format!("{}:{}", meta.len(), mtime))
}
fn verification_dir(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(LOCAL_VERIFICATION_DIR);
    crate::local_install::private_dir(&dir)?;
    Ok(dir)
}
/// Full SHA-256 of the file against the DECLARED digest. A size+mtime marker in
/// the private root remembers a previous full verification of the same path so
/// multi-GB files are not re-read on every view; any change invalidates it. The
/// spawn path (llama start_server) always re-hashes before the server runs.
pub fn verify_declared_digest(root: &Path, path: &Path, expected: &str) -> Result<bool, String> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Declared digest is not a SHA-256 hex string".into());
    }
    safe_regular_file(path)?;
    let current = marker_value(path).ok_or("Cannot stat model file")?;
    let key = hex::encode(Sha256::digest(
        format!("{expected}\n{}", path.display()).as_bytes(),
    ));
    let marker = verification_dir(root)
        .ok()
        .map(|d| d.join(format!("{key}.verified")));
    if let Some(m) = marker.as_ref().filter(|m| m.is_file()) {
        if fs::read_to_string(m)
            .map(|s| s.trim() == current)
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }
    let actual = sha256_file(path)?;
    if actual != expected {
        return Ok(false);
    }
    if let Some(m) = marker {
        // Best-effort cache only; a failed write just means re-hashing later.
        let _ = fs::write(&m, current);
    }
    Ok(true)
}

fn smoke_path(root: &Path, sha256: &str) -> Result<PathBuf, String> {
    Ok(verification_dir(root)?.join(format!("smoke-{}.json", sha256.to_ascii_lowercase())))
}
/// Persist this-device smoke evidence. Native-written, native-read; it is an
/// explanation of past evidence, never an authorization — memory and digest
/// are re-decided on every selection/start.
pub fn record_smoke(root: &Path, evidence: &SmokeEvidence) -> Result<(), String> {
    let path = smoke_path(root, &evidence.sha256)?;
    let bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    let pending = path.with_extension(format!("pending-{}", uuid::Uuid::new_v4()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|e| e.to_string())?;
    f.write_all(&bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(&pending, &path).map_err(|e| e.to_string())
}
pub fn smoke_evidence(root: &Path, sha256: &str, probe: &DeviceProbe) -> Option<SmokeEvidence> {
    let path = smoke_path(root, sha256).ok()?;
    safe_regular_file(&path).ok()?;
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .ok()?
        .take((MAX_WIRE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    let e: SmokeEvidence = decode(&bytes).ok()?;
    // Same digest, same OS and same total RAM: otherwise this is another device.
    (e.schema_version == 1
        && e.sha256.eq_ignore_ascii_case(sha256)
        && e.generated_tokens > 0
        && Some(&e.os) == probe.os.measured()
        && Some(&e.total_ram_bytes) == probe.total_ram_bytes.measured())
    .then_some(e)
}

/// Signed catalog/qualification payloads found under `<root>/model-qualification`.
/// Exact bytes are verified by the configured trust set (empty in shipping), so
/// an unsigned or self-made record never becomes a decision input.
#[derive(Clone, Debug, Default)]
pub struct SignedRecords {
    pub candidates: Vec<(Vec<u8>, Attestation)>,
    pub qualifications: Vec<(Vec<u8>, Attestation)>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedRecordFile {
    domain: String,
    payload_base64: String,
    attestation: Attestation,
}
pub fn load_signed_records(root: &Path) -> SignedRecords {
    let mut out = SignedRecords::default();
    let Ok(entries) = fs::read_dir(root.join(LOCAL_SIGNED_DIR)) else {
        return out;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths.into_iter().take(64) {
        if path.extension().and_then(|e| e.to_str()) != Some("json")
            || safe_regular_file(&path).is_err()
        {
            continue;
        }
        let mut bytes = Vec::new();
        if fs::File::open(&path)
            .ok()
            .and_then(|f| {
                f.take((MAX_WIRE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .ok()
            })
            .is_none()
        {
            continue;
        }
        let Ok(file) = decode::<SignedRecordFile>(&bytes) else {
            continue;
        };
        let Ok(payload) = STANDARD.decode(file.payload_base64.as_bytes()) else {
            continue;
        };
        if payload.len() > MAX_WIRE_BYTES {
            continue;
        }
        match file.domain.as_str() {
            "CATALOG_CANDIDATE_V1" => out.candidates.push((payload, file.attestation)),
            "QUALIFICATION_RECORD_V1" => out.qualifications.push((payload, file.attestation)),
            _ => {}
        }
    }
    out
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1073741824.0
}

/// THE local decision point. Called by native selection, native server start
/// and the device assessment with the same inputs. Whenever a verified signed
/// candidate pins this exact file digest, the core `evaluate()` result is the
/// decision (RECOMMENDED → qualified; UNSUPPORTED/not eligible → refused).
/// Without one — the shipping state — `evaluate()` cannot be constructed (the
/// core refuses unsigned candidates and a DETECTED-only backend by design), so
/// the works-here lane applies: declared digest verified, projector verified,
/// legacy tier/TOTAL-RAM rule, plus the AVAILABLE-RAM rule. Result labels never
/// say "qualified" unless a signed record did.
pub fn decide_local<V: SignatureVerifier>(
    root: &Path,
    file: &LocalModelFile,
    probe: &DeviceProbe,
    request: &LocalRequest,
    signed: &SignedRecords,
    verifier: &V,
    now: u64,
) -> LocalModelReport {
    let mut r = LocalModelReport {
        id: file.id.clone(),
        path: file.path.display().to_string(),
        mmproj_path: file.mmproj_path.as_ref().map(|p| p.display().to_string()),
        tier: crate::desktop_model_policy::tier(&file.id).map(|t| tier_name(t).to_owned()),
        present: file.path.is_file(),
        hash_verified: false,
        weights_bytes: fs::metadata(&file.path).ok().map(|m| m.len()),
        projector_bytes: None,
        kv_estimate_bytes: None,
        context_tokens: request.context_tokens,
        total_ram_bytes: probe.total_ram_bytes.measured().copied(),
        available_ram_bytes: probe.available_ram_bytes.measured().copied(),
        required_available_bytes: None,
        state: LocalState::Unknown,
        outcome: LocalOutcome::Unavailable,
        label: LABEL_UNKNOWN,
        reasons: vec![],
        decision: None,
        smoke: None,
    };
    if !r.present {
        r.reasons.push("Model file is not present on disk".into());
        return r;
    }
    let Some(expected) = file.expected_sha256.as_deref() else {
        r.reasons
            .push("No declared SHA-256 for this file (scanned, not declared)".into());
        return r;
    };
    let expected = expected.trim().to_ascii_lowercase();
    match verify_declared_digest(root, &file.path, &expected) {
        Ok(true) => r.hash_verified = true,
        Ok(false) => {
            r.outcome = LocalOutcome::Refused;
            r.reasons
                .push("Model bytes do not match the declared SHA-256".into());
            return r;
        }
        Err(e) => {
            r.reasons
                .push(format!("Digest verification unavailable: {e}"));
            return r;
        }
    }
    if let Some(mmproj) = file.mmproj_path.as_ref() {
        if !mmproj.is_file() {
            r.outcome = LocalOutcome::Refused;
            r.reasons
                .push("Declared vision projector for this tier is missing".into());
            return r;
        }
        match file.expected_mmproj_sha256.as_deref() {
            Some(h) => match verify_declared_digest(root, mmproj, h) {
                Ok(true) => {}
                Ok(false) => {
                    r.outcome = LocalOutcome::Refused;
                    r.reasons
                        .push("Projector bytes do not match the declared SHA-256".into());
                    return r;
                }
                Err(e) => {
                    r.reasons
                        .push(format!("Projector digest verification unavailable: {e}"));
                    return r;
                }
            },
            None => {
                r.reasons
                    .push("Declared projector has no SHA-256; cannot verify".into());
                return r;
            }
        }
        r.projector_bytes = fs::metadata(mmproj).ok().map(|m| m.len());
    }
    let Some(weights) = r.weights_bytes.filter(|w| *w > 0) else {
        r.reasons.push("Model file is empty".into());
        return r;
    };
    let (Some(total), Some(available)) = (r.total_ram_bytes, r.available_ram_bytes) else {
        r.reasons
            .push("Total/available RAM was not measured; no load decision".into());
        return r;
    };
    // KV estimate from the artifact's own header for the requested context
    // (clamped by trained context and host tier exactly like the launcher).
    let meta = crate::gguf_meta::read_metadata(&file.path).ok();
    let budget = crate::gguf_meta::derive_context_budget(
        meta.as_ref(),
        request.context_tokens,
        Some(total >> 30),
        request.cache_type_k.as_deref(),
    );
    r.context_tokens = budget.granted_context;
    r.kv_estimate_bytes = budget.kv_estimate_bytes;
    let kv = budget.kv_estimate_bytes.unwrap_or_else(|| {
        r.reasons.push(format!(
            "KV size not derivable from GGUF header; {} GiB assumed",
            gib(LOCAL_KV_FALLBACK_BYTES)
        ));
        LOCAL_KV_FALLBACK_BYTES
    });
    let projector = r.projector_bytes.unwrap_or(0);
    let required_available = weights
        .checked_add(projector)
        .and_then(|v| v.checked_add(kv))
        .and_then(|v| v.checked_add(LOCAL_AVAILABLE_RESERVE_BYTES));
    r.required_available_bytes = required_available;
    r.smoke = smoke_evidence(root, &expected, probe);

    // Signed lane: the core decides whenever a trusted candidate pins this digest.
    let mut signed_candidate: Option<VerifiedCandidate> = None;
    for (bytes, attestation) in &signed.candidates {
        if let Ok(c) = verify_candidate(bytes, attestation, verifier) {
            if c.candidate()
                .artifacts
                .iter()
                .any(|a| a.kind == ArtifactKind::Weights && a.sha256 == expected)
            {
                signed_candidate = Some(c);
                break;
            }
        }
    }
    if let Some(candidate) = signed_candidate {
        let quals: Vec<_> = signed
            .qualifications
            .iter()
            .filter_map(|(b, a)| validate_qualification(b, a, verifier, &candidate).ok())
            .collect();
        let memory = &candidate.candidate().memory;
        let core_request = AdmissionRequest {
            schema_version: SCHEMA_VERSION,
            capabilities: vec![Capability::Chat],
            languages: vec!["en".into()],
            context_tokens: memory.context_tokens,
            kv_format: memory.kv_format.clone(),
            parallel_agents: 1,
        };
        let d = evaluate(&candidate, probe, &core_request, &quals, None, now);
        r.reasons.push(format!(
            "Signed candidate {} v{}: core decision {:?} / {:?}",
            d.candidate_id, d.candidate_version, d.status, d.reasons
        ));
        r.context_tokens = d.context_tokens;
        let status = d.status.clone();
        let eligible = d.eligible_now;
        r.decision = Some(d);
        match status {
            DecisionStatus::Recommended => {
                r.state = LocalState::Qualified;
                r.outcome = LocalOutcome::Recommended;
                r.label = LABEL_QUALIFIED;
                return r;
            }
            DecisionStatus::SupportedWithLimits if eligible => {
                r.state = LocalState::Qualified;
                r.outcome = LocalOutcome::AllowedWithLimits;
                r.label = LABEL_QUALIFIED;
                return r;
            }
            DecisionStatus::SupportedWithLimits | DecisionStatus::Unsupported => {
                r.outcome = LocalOutcome::Refused;
                return r;
            }
            // NOT_YET_QUALIFIED: signed but not admitted — fall through to the
            // works-here lane; the record does not count against the file.
            DecisionStatus::NotYetQualified => {}
        }
    }

    // Works-here lane.
    let Some(tier) = crate::desktop_model_policy::tier(&file.id) else {
        r.reasons
            .push("Declared id is not an E2B/E4B/12B desktop tier".into());
        return r;
    };
    let Some(required_available) = required_available else {
        r.outcome = LocalOutcome::Refused;
        r.reasons.push("Memory arithmetic overflow".into());
        return r;
    };
    if !crate::desktop_model_policy::fits(tier, gib(total), gib(weights), gib(projector), gib(kv)) {
        r.outcome = LocalOutcome::Refused;
        r.reasons.push(format!(
            "{} tier estimate (weights {:.2} GiB ×1.1 + projector {:.2} + KV {:.2} + 1 GiB) exceeds the {:.1} GiB TOTAL RAM budget or tier minimum",
            tier_name(tier), gib(weights), gib(projector), gib(kv), gib(total)
        ));
        return r;
    }
    if required_available > available {
        r.outcome = LocalOutcome::Refused;
        r.reasons.push(format!(
            "Insufficient AVAILABLE RAM now: need {:.2} GiB (weights + projector + KV + 1 GiB reserve), {:.2} GiB available",
            gib(required_available),
            gib(available)
        ));
        return r;
    }
    r.outcome = LocalOutcome::AllowedWithLimits;
    if r.smoke.is_some() {
        r.state = LocalState::WorksHere;
        r.label = LABEL_WORKS_HERE;
        r.reasons.push(
            "Native load + inference smoke previously passed on this device for this digest; no signed qualification".into(),
        );
    } else {
        r.state = LocalState::Unknown;
        r.label = LABEL_FITS_UNTESTED;
        r.reasons.push(
            "Fits TOTAL and AVAILABLE RAM estimates; load allowed, becomes 'works here' only after a real inference smoke".into(),
        );
    }
    r
}

/// Server replacement rule shared by the native start path: a candidate server
/// is promoted only after its inference smoke passes; otherwise it is killed and
/// the previous server (if any) stays active and published.
#[derive(Debug, PartialEq, Eq)]
pub enum Replacement<T> {
    Promote {
        stop_previous: Option<T>,
        active: T,
    },
    Rollback {
        kill_candidate: T,
        active: Option<T>,
    },
}
pub fn resolve_replacement<T>(
    previous: Option<T>,
    candidate: T,
    smoke_passed: bool,
) -> Replacement<T> {
    if smoke_passed {
        Replacement::Promote {
            stop_previous: previous,
            active: candidate,
        }
    } else {
        Replacement::Rollback {
            kill_candidate: candidate,
            active: previous,
        }
    }
}

/// One short, bounded completion against the loopback llama-server. Passes only
/// when HTTP 200, the first choice carries non-empty text and (when reported) a
/// positive completion token count. Any error, timeout, empty answer or OOM-style
/// failure is a smoke FAILURE — the caller must not mark the server ready.
pub async fn inference_smoke(
    port: u16,
    model_id: &str,
    timeout: std::time::Duration,
) -> Result<(u64, u64, u64), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "model": model_id,
        "messages": [{"role": "user", "content": "Reply with the single word OK."}],
        "max_tokens": 8,
        "temperature": 0,
        "stream": false,
    });
    let started = std::time::Instant::now();
    let response = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Smoke request failed: {e}"))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Smoke response unreadable: {e}"))?;
    let elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if !status.is_success() {
        return Err(format!("Smoke completion returned HTTP {status}"));
    }
    if bytes.len() > MAX_WIRE_BYTES {
        return Err("Smoke response exceeds bound".into());
    }
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("Smoke response is not JSON: {e}"))?;
    let content = json
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if content.is_empty() {
        return Err("Smoke completion returned no text".into());
    }
    let tokens = match json
        .pointer("/usage/completion_tokens")
        .and_then(|v| v.as_u64())
    {
        Some(0) => return Err("Smoke completion reported zero generated tokens".into()),
        Some(n) => n,
        None => 1,
    };
    Ok((tokens, elapsed_ms.max(1), content.chars().count() as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    #[test]
    fn real_ed25519_exact_payload_domain_pins_and_revocation() {
        // SYNTHETIC TEST KEY ONLY. Never part of shipping trust configuration.
        let pair = Ed25519KeyPair::from_seed_unchecked(&[42; 32]).unwrap();
        let payload = br#"{"fixture":true}"#;
        let verifier = RingVerifier::configured(vec![TrustedPayload {
            domain: SignedDomain::CatalogCandidateV1,
            key_id: "fixture".into(),
            public_key: pair.public_key().as_ref().try_into().unwrap(),
            payload_sha256: Sha256::digest(payload).into(),
        }]);
        let mut a = Attestation {
            key_id: "fixture".into(),
            algorithm: "Ed25519".into(),
            signature: STANDARD.encode(pair.sign(payload).as_ref()),
        };
        assert!(verifier
            .verify(SignedDomain::CatalogCandidateV1, payload, &a)
            .is_ok());
        assert!(verifier
            .verify(SignedDomain::QualificationRecordV1, payload, &a)
            .is_err());
        assert!(verifier
            .verify(SignedDomain::CatalogCandidateV1, b"{}", &a)
            .is_err());
        assert!(RingVerifier::shipping()
            .verify(SignedDomain::CatalogCandidateV1, payload, &a)
            .is_err());
        a.signature = STANDARD.encode([0; 64]);
        assert!(verifier
            .verify(SignedDomain::CatalogCandidateV1, payload, &a)
            .is_err());
    }
    #[test]
    fn real_signed_fixture_candidate_and_qualification_use_exact_separate_domains() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../packages/model-admission/tests/fixtures/admission-v1.json"
        ))
        .unwrap();
        let c = serde_json::to_vec(&fixture["candidate"]).unwrap();
        let q = serde_json::to_vec(&fixture["qualification"]).unwrap();
        let pair = Ed25519KeyPair::from_seed_unchecked(&[44; 32]).unwrap(); // fixture only
        let key: [u8; 32] = pair.public_key().as_ref().try_into().unwrap();
        let verifier = RingVerifier::configured(vec![
            TrustedPayload {
                domain: SignedDomain::CatalogCandidateV1,
                key_id: "fixture".into(),
                public_key: key,
                payload_sha256: Sha256::digest(&c).into(),
            },
            TrustedPayload {
                domain: SignedDomain::QualificationRecordV1,
                key_id: "fixture".into(),
                public_key: key,
                payload_sha256: Sha256::digest(&q).into(),
            },
        ]);
        let sign = |b: &[u8]| Attestation {
            key_id: "fixture".into(),
            algorithm: "Ed25519".into(),
            signature: STANDARD.encode(pair.sign(b).as_ref()),
        };
        let candidate = verify_candidate(&c, &sign(&c), &verifier).unwrap();
        assert!(validate_qualification(&q, &sign(&q), &verifier, &candidate).is_ok());
        assert!(validate_qualification(&q, &sign(&c), &verifier, &candidate).is_err());
    }
    #[test]
    fn shipping_gate_never_claims_ready() {
        assert!(require_shipping_admission().is_err());
    }
    #[test]
    fn real_local_consent_is_expiring_revocable_and_not_json_authority() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../packages/model-admission/tests/fixtures/admission-v1.json"
        ))
        .unwrap();
        let mut policy: StandingDownloadPolicy =
            serde_json::from_value(fixture["policy"].clone()).unwrap();
        policy.not_before_ms = crate::provisioning_probe::now_ms() - 1000;
        policy.expires_at_ms = crate::provisioning_probe::now_ms() + 60_000;
        let store = LocalConsentStore::open(dir.path()).unwrap();
        assert!(NativePolicyGrant::from_local_store(policy.clone(), &store).is_err());
        store.approve_from_native(&policy).unwrap();
        assert!(store.grant().is_ok());
        let old = store.grant().unwrap();
        store.revoke().unwrap();
        assert!(store.grant().is_err());
        assert!(store.validate_local_approval(old.policy()).is_err());
        policy.expires_at_ms = policy.not_before_ms + 1;
        store.approve_from_native(&policy).unwrap();
        assert_eq!(store.grant().unwrap_err(), ContractError::Expired);
    }
}
