//! Encrypted Stage 2 evidence repository in the existing product vault.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use unoone_capability_contracts::knowledge::*;
use unoone_vault_core::{EncryptedRecord, PrivacyLevel, Record, RecordType, Vault, VaultError};

pub const CATALOG_SCHEMA: &str = "inbharat.pai.evidence-vault.catalog";
pub const CATALOG_SCHEMA_VERSION: u32 = 2;
pub const MAX_CATALOG_ENTRIES: usize = 8192;
pub const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_GRAPH_DEPTH: usize = 128;
const ORIGIN: &str = "PAI";
const DEVICE: &str = "LOCAL";

/// Error messages deliberately contain no record payload or source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeStoreError {
    Locked,
    Uninitialized,
    Conflict,
    Invalidated,
    NotFound,
    InvalidRecord,
    Corrupt,
    UnsupportedSchema,
    Limit,
    Persistence,
}
impl std::fmt::Display for KnowledgeStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "knowledge store: {self:?}")
    }
}
impl std::error::Error for KnowledgeStoreError {}
pub(crate) type Result<T> = std::result::Result<T, KnowledgeStoreError>;

/// Returned to Stage 3/4 only after encrypted records and their references have
/// been read/validated. These fields are persisted inside encryption, never as
/// vault plaintext metadata. A physical UUID is an opaque random storage key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordMapping {
    pub reference: RecordRef,
    pub physical_uuid: String,
    pub source_id: String,
    pub source_version: String,
    pub source_commit: String,
    pub previous: Option<RecordRef>,
    pub references: Vec<RecordRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredKnowledge {
    pub mapping: RecordMapping,
    pub record: KnowledgeRecord,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Catalog {
    schema: String,
    schema_version: u32,
    pub(crate) vault_id: String,
    pub(crate) generation: u32,
    pub(crate) entries: Vec<RecordMapping>,
    #[serde(default)]
    binding_tag: String,
}

/// An immutable encrypted initialization marker makes deletion of the catalog
/// distinguishable from a never-initialized store across reopen. It is another
/// generic Private record in the SAME vault, not another database/index.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogAnchor {
    schema: String,
    schema_version: u32,
    vault_id: String,
    catalog_uuid: String,
    #[serde(default)]
    binding_tag: String,
}

/// Product metadata repository, deliberately NOT a MemoryProvider, executor,
/// model verifier or promotion policy. All methods serialize operations using
/// the SAME Arc<Mutex<Option<Vault>>> as the owning product vault adapter.
/// No plaintext topic/path/catalog file is created and no vault directory scan
/// or conversation ingestion is performed.
pub struct EvidenceVault {
    vault: Arc<Mutex<Option<Vault>>>,
}

impl EvidenceVault {
    pub fn new(vault: Arc<Mutex<Option<Vault>>>) -> Self {
        Self { vault }
    }
    pub fn catalog_uuid() -> String {
        namespaced_uuid(b"inbharat.pai.evidence-vault.catalog.v1\0")
    }
    pub fn anchor_uuid() -> String {
        namespaced_uuid(b"inbharat.pai.evidence-vault.catalog-anchor.v1\0")
    }
    pub(crate) fn with_vault<T>(
        &self,
        operation: impl FnOnce(&mut Vault) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self
            .vault
            .lock()
            .map_err(|_| KnowledgeStoreError::Persistence)?;
        let vault = guard.as_mut().ok_or(KnowledgeStoreError::Locked)?;
        if !vault.is_unlocked() {
            return Err(KnowledgeStoreError::Locked);
        }
        operation(vault)
    }

    /// Explicit create-only initialization. An occupied UUID, old/future schema,
    /// tombstone, malformed catalog or lone anchor is NEVER overwritten/repaired.
    /// An existing valid initialized store is idempotently read, not rewritten.
    pub fn initialize(&self) -> Result<()> {
        self.with_vault(|vault| {
            let catalog = optional_record(vault, &Self::catalog_uuid())?;
            let anchor = optional_record(vault, &Self::anchor_uuid())?;
            match (catalog, anchor) {
                (Some(_), Some(_)) => {
                    let (catalog, _) = load_catalog(vault)?;
                    load_all(vault, &catalog)?;
                    Ok(())
                }
                (None, None) => {
                    let vault_id = vault
                        .vault_id()
                        .ok_or(KnowledgeStoreError::Corrupt)?
                        .to_owned();
                    let mut anchor = CatalogAnchor {
                        schema: format!("{CATALOG_SCHEMA}.anchor"),
                        schema_version: CATALOG_SCHEMA_VERSION,
                        vault_id: vault_id.clone(),
                        catalog_uuid: Self::catalog_uuid(),
                        binding_tag: String::new(),
                    };
                    anchor.binding_tag = sign_binding(
                        vault,
                        ControlDomain::CatalogAnchor,
                        &control_bytes(&anchor)?,
                    )?;
                    let bytes = control_bytes(&anchor)?;
                    let mut catalog = Catalog {
                        schema: CATALOG_SCHEMA.into(),
                        schema_version: CATALOG_SCHEMA_VERSION,
                        vault_id,
                        generation: 1,
                        entries: vec![],
                        binding_tag: String::new(),
                    };
                    seal_catalog(vault, &mut catalog)?;
                    let catalog_payload = catalog_bytes(&catalog)?;
                    // Both complete signed controls are prepared before any write.
                    write_new(vault, &Self::anchor_uuid(), &bytes)?;
                    write_new(vault, &Self::catalog_uuid(), &catalog_payload)?;
                    load_catalog(vault)?;
                    Ok(())
                }
                _ => Err(KnowledgeStoreError::Corrupt),
            }
        })
    }

    pub fn create(&self, record: KnowledgeRecord) -> Result<RecordMapping> {
        self.persist(record, None)
    }

    /// Optimistic revision gate plus append-only physical writes; old bytes and
    /// audit headers survive. Candidate -> VerifiedPattern -> ApprovedProcedure
    /// transitions are recorded metadata, NOT runtime approval or execution.
    pub fn append_revision(
        &self,
        record: KnowledgeRecord,
        expected_previous: &RecordRef,
    ) -> Result<RecordMapping> {
        self.persist(record, Some(expected_previous))
    }

    /// Explicit append-only invalidation; never calls vault.delete_record.
    /// The exact existing target is authenticated as audit history, even when
    /// superseded/already revoked. A distinct ID can record another revocation;
    /// reusing an audit ID conflicts. All extra graph references stay active-only.
    pub fn invalidate(&self, record: KnowledgeRecord) -> Result<RecordMapping> {
        if !matches!(record, KnowledgeRecord::Invalidation(_)) {
            return Err(KnowledgeStoreError::InvalidRecord);
        }
        self.create(record)
    }

    fn persist(
        &self,
        record: KnowledgeRecord,
        expected: Option<&RecordRef>,
    ) -> Result<RecordMapping> {
        let bytes = record
            .encode()
            .map_err(|_| KnowledgeStoreError::InvalidRecord)?;
        if let KnowledgeRecord::Evidence(e) = &record {
            if digest(e.content.as_bytes()) != e.content_sha256 {
                return Err(KnowledgeStoreError::InvalidRecord);
            }
        }
        self.with_vault(|vault| {
            let (mut catalog, mut metadata) = load_catalog(vault)?;
            // Validate existing history/graph before ANY write. Missing/deleted or
            // corrupted referenced blobs cannot be ignored by a new transition.
            let loaded = load_all(vault, &catalog)?;
            let h = record.header();
            let latest = catalog
                .entries
                .iter()
                .rev()
                .find(|e| e.reference.logical_id == h.logical_id);
            match (expected, latest) {
                (None, None) if h.revision == 1 && h.previous.is_none() => {}
                (Some(expected), Some(current))
                    if &current.reference == expected
                        && h.previous.as_ref() == Some(expected)
                        && expected.revision.checked_add(1) == Some(h.revision)
                        && allowed_transition(expected.kind, record.kind()) => {}
                _ => return Err(KnowledgeStoreError::Conflict),
            }
            // Audit authentication is mandatory for EVERY exact reference.
            // Revocation must not depend on the target's active admission: old
            // provenance remains revocable after superseding or prior revocation.
            // load_all has already authenticated every published physical blob.
            for reference in record.references() {
                require_reference(&loaded, reference)?;
            }
            if let KnowledgeRecord::Invalidation(x) = &record {
                // Only the typed target gets audit-only admission; other graph
                // references retain the ordinary active-only dependency gate.
                for edge in &x.header.edges {
                    require_active(&loaded, &edge.target)?;
                }
            } else {
                for reference in record.references() {
                    require_active(&loaded, reference)?;
                }
            }
            validate_edge_semantics(&record, &loaded)?;
            if catalog.entries.len() >= MAX_CATALOG_ENTRIES {
                return Err(KnowledgeStoreError::Limit);
            }
            // Record::new supplies a random UUID. Do not derive paths from source,
            // topic, logical ID or payload, and never overwrite a collision.
            let mut physical = private_metadata();
            let mapping = mapping_for(&record, physical.record_id.clone(), digest(&bytes));
            if physical.record_id == Self::catalog_uuid()
                || physical.record_id == Self::anchor_uuid()
                || optional_record(vault, &physical.record_id)?.is_some()
            {
                return Err(KnowledgeStoreError::Conflict);
            }
            catalog.entries.push(mapping.clone());
            catalog.generation = catalog
                .generation
                .checked_add(1)
                .ok_or(KnowledgeStoreError::Limit)?;
            // Fixed-size tag keeps preflight exact; signing occurs only after
            // new payload readback, over the full verified snapshot under this lock.
            catalog.binding_tag = "0".repeat(64);
            catalog_bytes(&catalog)?; // preflight before data write
            physical.privacy_level = PrivacyLevel::Private;
            vault.write_record(physical, &bytes).map_err(map_error)?;
            read_mapping(vault, &mapping)?;
            seal_catalog(vault, &mut catalog)?;
            let catalog_payload = catalog_bytes(&catalog)?;
            // Only a previously strictly validated catalog is mutable. No generic
            // record at this UUID can be overwritten. Mutex guards stale writers.
            metadata.privacy_level = PrivacyLevel::Private;
            vault
                .write_record(metadata, &catalog_payload)
                .map_err(map_error)?;
            let (committed, _) = load_catalog(vault)?;
            if committed.entries.last() != Some(&mapping) {
                return Err(KnowledgeStoreError::Corrupt);
            }
            Ok(mapping)
        })
    }

    /// Authenticate the source-published v2 catalog before selecting payloads.
    /// Reads only candidates plus transitive/incoming closure, without a cache.
    /// Unrelated payload corruption is left to full audit/mutation APIs; closure
    /// can still encompass the whole bounded graph. No keyholder/rollback defense.
    pub fn read_targeted(&self, references: &[RecordRef]) -> Result<TargetedRead> {
        if references.len() > MAX_TARGETED_CANDIDATES {
            return Err(KnowledgeStoreError::Limit);
        }
        for reference in references {
            reference
                .validate()
                .map_err(|_| KnowledgeStoreError::InvalidRecord)?;
        }
        self.with_vault(|vault| {
            let (catalog, _) = load_catalog(vault)?;
            read_selected(vault, &catalog, references)
        })
    }

    /// Complete bounded mappings for Stage 3's future encrypted postings index.
    /// No application policy, model ranking or execution permission is implied.
    pub fn catalog(&self) -> Result<Vec<RecordMapping>> {
        self.with_vault(|vault| {
            let (catalog, _) = load_catalog(vault)?;
            load_all(vault, &catalog)?;
            Ok(catalog.entries)
        })
    }
    pub fn history(&self, logical_id: &str) -> Result<Vec<RecordMapping>> {
        valid_id(logical_id).map_err(|_| KnowledgeStoreError::InvalidRecord)?;
        Ok(self
            .catalog()?
            .into_iter()
            .filter(|e| e.reference.logical_id == logical_id)
            .collect())
    }
    /// Immutable audit read: invalidated records are still readable, but malformed
    /// graphs, missing references and hash/kind/identity mismatches fail closed.
    pub fn read(&self, reference: &RecordRef) -> Result<StoredKnowledge> {
        self.read_internal(reference, false)
    }
    pub fn read_active(&self, reference: &RecordRef) -> Result<StoredKnowledge> {
        self.read_internal(reference, true)
    }
    fn read_internal(&self, reference: &RecordRef, active: bool) -> Result<StoredKnowledge> {
        reference
            .validate()
            .map_err(|_| KnowledgeStoreError::InvalidRecord)?;
        self.with_vault(|vault| {
            let (catalog, _) = load_catalog(vault)?;
            let loaded = load_all(vault, &catalog)?;
            let item = require_reference(&loaded, reference)?;
            if active {
                require_active(&loaded, reference)?;
            }
            Ok(item.clone())
        })
    }
    pub fn read_latest(&self, logical_id: &str) -> Result<StoredKnowledge> {
        valid_id(logical_id).map_err(|_| KnowledgeStoreError::InvalidRecord)?;
        self.with_vault(|vault| {
            let (catalog, _) = load_catalog(vault)?;
            let loaded = load_all(vault, &catalog)?;
            let mapping = catalog
                .entries
                .iter()
                .rev()
                .find(|e| e.reference.logical_id == logical_id)
                .ok_or(KnowledgeStoreError::NotFound)?;
            Ok(require_reference(&loaded, &mapping.reference)?.clone())
        })
    }
    /// Stored edges plus the inverse invalidated_by edges derived from immutable
    /// invalidation records. No evidence/pattern bytes are rewritten to revoke.
    /// Every returned target has already been read and kind/hash/identity checked.
    pub fn graph_edges(&self, reference: &RecordRef) -> Result<Vec<GraphEdge>> {
        reference
            .validate()
            .map_err(|_| KnowledgeStoreError::InvalidRecord)?;
        self.with_vault(|vault| {
            let (catalog, _) = load_catalog(vault)?;
            let loaded = load_all(vault, &catalog)?;
            let item = require_reference(&loaded, reference)?;
            let mut edges = item.record.header().edges.clone();
            for invalidation in loaded.values() {
                if let KnowledgeRecord::Invalidation(x) = &invalidation.record {
                    if &x.target == reference {
                        edges.push(GraphEdge {
                            relation: EdgeKind::InvalidatedBy,
                            target: invalidation.mapping.reference.clone(),
                        });
                    }
                }
            }
            Ok(edges)
        })
    }
    /// Includes transitive revocation/superseding dependencies; not an execution gate.
    pub fn is_invalidated(&self, reference: &RecordRef) -> Result<bool> {
        match self.read_active(reference) {
            Ok(_) => Ok(false),
            Err(KnowledgeStoreError::Invalidated) => Ok(true),
            Err(e) => Err(e),
        }
    }
}

/// Candidate cap bounds output; dependency/incoming closure can be larger.
pub const MAX_TARGETED_CANDIDATES: usize = 64;
#[derive(Debug, Clone, PartialEq)]
pub struct TargetedKnowledge {
    pub stored: StoredKnowledge,
    pub active: bool,
    /// Both incoming and outgoing contradicting facts are surfaced, never resolved
    /// as active advice by a retrieval heuristic.
    pub contradictory: bool,
    /// Authenticated transitive source scopes, without dependency payload bodies.
    pub dependency_metadata: Vec<KnowledgeMetadata>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TargetedRead {
    pub catalog_generation: u32,
    pub items: Vec<TargetedKnowledge>,
    pub payload_decryptions: usize,
}

pub(crate) fn read_selected(
    vault: &Vault,
    catalog: &Catalog,
    references: &[RecordRef],
) -> Result<TargetedRead> {
    if references.len() > MAX_TARGETED_CANDIDATES {
        return Err(KnowledgeStoreError::Limit);
    }
    // load_catalog has authenticated the complete mapping vectors with a
    // separate publication MAC. Absence of incoming edges is now trustworthy
    // within the trusted-controller boundary, not inferred from AEAD alone.
    let mappings: BTreeMap<_, _> = catalog
        .entries
        .iter()
        .map(|m| (m.reference.clone(), m))
        .collect();
    let mut needed = BTreeSet::new();
    let mut pending = references.to_vec();
    while let Some(reference) = pending.pop() {
        let mapping = mappings.get(&reference).ok_or_else(|| {
            if catalog.entries.iter().any(|m| {
                m.reference.logical_id == reference.logical_id
                    && m.reference.revision == reference.revision
            }) {
                KnowledgeStoreError::Corrupt
            } else {
                KnowledgeStoreError::NotFound
            }
        })?;
        if needed.insert(reference.clone()) {
            if needed.len() > MAX_CATALOG_ENTRIES {
                return Err(KnowledgeStoreError::Limit);
            }
            pending.extend(mapping.references.iter().cloned());
            // Include all incoming facts (not only Invalidation kind), then
            // recurse through their dependencies and incoming facts as well.
            pending.extend(
                catalog
                    .entries
                    .iter()
                    .filter(|m| m.references.contains(&reference))
                    .map(|m| m.reference.clone()),
            );
        }
    }
    let mut loaded = Loaded::new();
    for mapping in catalog
        .entries
        .iter()
        .filter(|m| needed.contains(&m.reference))
    {
        let item = read_mapping(vault, mapping)?;
        for reference in item.record.references() {
            require_reference(&loaded, reference)?;
        }
        validate_edge_semantics(&item.record, &loaded)?;
        loaded.insert(
            (
                mapping.reference.logical_id.clone(),
                mapping.reference.revision,
            ),
            item,
        );
    }
    selected_from_loaded(catalog, &loaded, references)
}

pub(crate) fn selected_from_loaded(
    catalog: &Catalog,
    loaded: &Loaded,
    references: &[RecordRef],
) -> Result<TargetedRead> {
    let mut items = Vec::new();
    for r in references {
        let stored = require_reference(loaded, r)?.clone();
        let active = match require_active(loaded, r) {
            Ok(()) => true,
            Err(KnowledgeStoreError::Invalidated) => false,
            Err(e) => return Err(e),
        };
        let mut dependencies = BTreeSet::new();
        let mut pending = vec![(r.clone(), 0usize)];
        let mut dependency_metadata = Vec::new();
        while let Some((reference, depth)) = pending.pop() {
            if depth >= MAX_GRAPH_DEPTH {
                return Err(KnowledgeStoreError::Limit);
            }
            if dependencies.insert(reference.clone()) {
                let item = require_reference(loaded, &reference)?;
                dependency_metadata.push(item.record.header().metadata.clone());
                pending.extend(
                    item.record
                        .references()
                        .iter()
                        .map(|r| ((*r).clone(), depth + 1)),
                );
            }
        }
        let contradictory = loaded.values().any(|item| {
            item.record.header().edges.iter().any(|e| {
                e.relation == EdgeKind::Contradicting
                    && (dependencies.contains(&e.target)
                        || dependencies.contains(&item.mapping.reference))
            })
        });
        items.push(TargetedKnowledge {
            stored,
            active,
            contradictory,
            dependency_metadata,
        });
    }
    Ok(TargetedRead {
        catalog_generation: catalog.generation,
        items,
        payload_decryptions: loaded.len(),
    })
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn namespaced_uuid(namespace: &[u8]) -> String {
    let digest = Sha256::digest(namespace);
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}
pub(crate) fn private_metadata() -> Record {
    Record::new(RecordType::Memory, ORIGIN, DEVICE)
}
pub(crate) fn map_error(error: VaultError) -> KnowledgeStoreError {
    match error {
        VaultError::VaultLocked => KnowledgeStoreError::Locked,
        VaultError::Io(_) => KnowledgeStoreError::Persistence,
        _ => KnowledgeStoreError::Corrupt,
    }
}
// Stage 2 writes only canonical AAD v2. Do not accept the vault's legacy
// compatibility path (or future AAD versions) for these new records: a caller
// must not downgrade aad_version to bypass metadata authentication.
pub(crate) fn strict_read(vault: &Vault, uuid: &str) -> Result<(Record, Vec<u8>)> {
    unoone_vault_core::vault::validate_record_id(uuid).map_err(|_| KnowledgeStoreError::Corrupt)?;
    let path = vault
        .vault_root()
        .join("VAULT/records")
        .join(format!("{uuid}.enc.json"));
    let size = std::fs::metadata(&path)
        .map_err(|_| KnowledgeStoreError::Persistence)?
        .len();
    if size > (MAX_CATALOG_BYTES * 3 + 16 * 1024) as u64 {
        return Err(KnowledgeStoreError::Limit);
    }
    let raw = std::fs::read(path).map_err(|_| KnowledgeStoreError::Persistence)?;
    let envelope: EncryptedRecord =
        serde_json::from_slice(&raw).map_err(|_| KnowledgeStoreError::Corrupt)?;
    if envelope.aad_version != unoone_vault_core::record::AAD_VERSION_CANONICAL {
        return Err(KnowledgeStoreError::Corrupt);
    }
    let result = vault.read_record(uuid).map_err(map_error)?;
    #[cfg(test)]
    READ_COUNTS.with(|counts| {
        let controls = [
            EvidenceVault::catalog_uuid(),
            EvidenceVault::anchor_uuid(),
            crate::knowledge_retrieval::KnowledgeRetriever::index_uuid(),
            crate::knowledge_retrieval::KnowledgeRetriever::anchor_uuid(),
        ];
        counts.borrow_mut()[usize::from(controls.iter().any(|id| id == uuid))] += 1;
    });
    Ok(result)
}
pub(crate) fn optional_record(vault: &Vault, uuid: &str) -> Result<Option<(Record, Vec<u8>)>> {
    // A genuine absence is the ONLY tolerated error. Tombstones, ciphertext
    // corruption, foreign occupied UUIDs and unsupported envelopes remain errors.
    unoone_vault_core::vault::validate_record_id(uuid).map_err(|_| KnowledgeStoreError::Corrupt)?;
    let records = vault.vault_root().join("VAULT/records");
    let path = records.join(format!("{uuid}.enc.json"));
    match std::fs::metadata(&path) {
        Ok(_) => strict_read(vault, uuid).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && records.is_dir() => Ok(None),
        Err(_) => Err(KnowledgeStoreError::Persistence),
    }
}
pub(crate) fn check_metadata(metadata: &Record, uuid: &str, bytes: &[u8]) -> Result<()> {
    if metadata.record_id != uuid
        || metadata.record_type != RecordType::Memory
        || metadata.privacy_level != PrivacyLevel::Private
        || metadata.schema_version != 1
        || metadata.encryption_version != 1
        || metadata.tombstone
        || metadata.deleted_at.is_some()
        || metadata.parent_record_id.is_some()
        || !metadata.source_record_ids.is_empty()
        || metadata.origin_platform != ORIGIN
        || metadata.origin_device_id != DEVICE
        || metadata.content_hash != digest(bytes)
    {
        return Err(KnowledgeStoreError::Corrupt);
    }
    Ok(())
}
pub(crate) fn write_new(vault: &mut Vault, uuid: &str, bytes: &[u8]) -> Result<()> {
    if optional_record(vault, uuid)?.is_some() {
        return Err(KnowledgeStoreError::Conflict);
    }
    let mut metadata = private_metadata();
    metadata.record_id = uuid.into();
    vault.write_record(metadata, bytes).map_err(map_error)?;
    let (metadata, readback) = strict_read(vault, uuid)?;
    check_metadata(&metadata, uuid, &readback)?;
    if readback != bytes || metadata.revision != 2 {
        return Err(KnowledgeStoreError::Corrupt);
    }
    Ok(())
}
// Publication authority is intentionally private to these controllers. General
// Vault::write_record authenticates encryption, but never invokes this MAC path.
// A master-key holder can derive these keys: this is NOT adversarial-owner or
// consistent-snapshot rollback protection. Stable v1 UUIDs are retained so old
// controls fail closed in place rather than being silently reset in a v2 namespace.
#[derive(Clone, Copy)]
pub(crate) enum ControlDomain {
    Catalog,
    CatalogAnchor,
    Index,
    IndexAnchor,
}
impl ControlDomain {
    fn name(self) -> &'static str {
        match self {
            Self::Catalog => "inbharat.pai.publication.catalog.hmac-sha256.v2",
            Self::CatalogAnchor => "inbharat.pai.publication.catalog-anchor.hmac-sha256.v2",
            Self::Index => "inbharat.pai.publication.postings.hmac-sha256.v2",
            Self::IndexAnchor => "inbharat.pai.publication.postings-anchor.hmac-sha256.v2",
        }
    }
}
pub(crate) fn control_bytes<T: Serialize>(control: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(control).map_err(|_| KnowledgeStoreError::Corrupt)
}
fn binding_mac(vault: &Vault, domain: ControlDomain, bytes: &[u8]) -> Result<Hmac<Sha256>> {
    let master = vault.master_key().ok_or(KnowledgeStoreError::Locked)?;
    let mut key = unoone_vault_core::crypto::derive_domain_key(master, domain.name());
    let result = Hmac::<Sha256>::new_from_slice(&key).map_err(|_| KnowledgeStoreError::Corrupt);
    unoone_vault_core::crypto::secure_zero(&mut key);
    let mut mac = result?;
    mac.update(b"inbharat.pai.control-publication\0hmac-sha256\0v2\0");
    mac.update(domain.name().as_bytes());
    mac.update(b"\0");
    mac.update(bytes);
    Ok(mac)
}
pub(crate) fn sign_binding(
    vault: &Vault,
    domain: ControlDomain,
    unsigned: &[u8],
) -> Result<String> {
    let tag = binding_mac(vault, domain, unsigned)?
        .finalize()
        .into_bytes();
    Ok(tag.iter().map(|b| format!("{b:02x}")).collect())
}
pub(crate) fn verify_binding(
    vault: &Vault,
    domain: ControlDomain,
    unsigned: &[u8],
    tag: &str,
) -> Result<()> {
    // Strict fixed-size lowercase encoding, then audited constant-time MAC API.
    if tag.len() != 64
        || !tag
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(KnowledgeStoreError::Corrupt);
    }
    let mut decoded = [0u8; 32];
    for (i, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&tag[i * 2..i * 2 + 2], 16)
            .map_err(|_| KnowledgeStoreError::Corrupt)?;
    }
    binding_mac(vault, domain, unsigned)?
        .verify_slice(&decoded)
        .map_err(|_| KnowledgeStoreError::Corrupt)
}
pub(crate) fn seal_catalog(vault: &Vault, catalog: &mut Catalog) -> Result<()> {
    catalog.binding_tag.clear();
    catalog.binding_tag = sign_binding(vault, ControlDomain::Catalog, &catalog_bytes(catalog)?)?;
    Ok(())
}
#[cfg(test)]
thread_local! {
    static READ_COUNTS: std::cell::RefCell<[usize; 2]> = const { std::cell::RefCell::new([0, 0]) };
}
#[cfg(test)]
pub(crate) fn reset_read_counts() {
    READ_COUNTS.with(|c| *c.borrow_mut() = [0, 0]);
}
#[cfg(test)]
pub(crate) fn read_counts() -> [usize; 2] {
    READ_COUNTS.with(|c| *c.borrow())
}

pub(crate) fn catalog_bytes(catalog: &Catalog) -> Result<Vec<u8>> {
    if catalog.entries.len() > MAX_CATALOG_ENTRIES {
        return Err(KnowledgeStoreError::Limit);
    }
    let bytes = serde_json::to_vec(catalog).map_err(|_| KnowledgeStoreError::InvalidRecord)?;
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err(KnowledgeStoreError::Limit);
    }
    Ok(bytes)
}
pub(crate) fn load_catalog(vault: &Vault) -> Result<(Catalog, Record)> {
    let raw = optional_record(vault, &EvidenceVault::catalog_uuid())?;
    let anchor_raw = optional_record(vault, &EvidenceVault::anchor_uuid())?;
    let ((metadata, bytes), (anchor_metadata, anchor_bytes)) = match (raw, anchor_raw) {
        (None, None) => return Err(KnowledgeStoreError::Uninitialized),
        (Some(c), Some(a)) => (c, a),
        _ => return Err(KnowledgeStoreError::Corrupt),
    };
    if bytes.len() > MAX_CATALOG_BYTES || anchor_bytes.len() > 1024 {
        return Err(KnowledgeStoreError::Limit);
    }
    check_metadata(&metadata, &EvidenceVault::catalog_uuid(), &bytes)?;
    check_metadata(
        &anchor_metadata,
        &EvidenceVault::anchor_uuid(),
        &anchor_bytes,
    )?;
    let anchor: CatalogAnchor =
        serde_json::from_slice(&anchor_bytes).map_err(|_| KnowledgeStoreError::Corrupt)?;
    let catalog: Catalog =
        serde_json::from_slice(&bytes).map_err(|_| KnowledgeStoreError::Corrupt)?;
    if catalog.schema != CATALOG_SCHEMA
        || catalog.schema_version != CATALOG_SCHEMA_VERSION
        || anchor.schema != format!("{CATALOG_SCHEMA}.anchor")
        || anchor.schema_version != CATALOG_SCHEMA_VERSION
    {
        return Err(KnowledgeStoreError::UnsupportedSchema);
    }
    if control_bytes(&anchor)? != anchor_bytes || catalog_bytes(&catalog)? != bytes {
        return Err(KnowledgeStoreError::Corrupt);
    }
    let mut unsigned_anchor = anchor.clone();
    unsigned_anchor.binding_tag.clear();
    verify_binding(
        vault,
        ControlDomain::CatalogAnchor,
        &control_bytes(&unsigned_anchor)?,
        &anchor.binding_tag,
    )?;
    let mut unsigned_catalog = catalog.clone();
    unsigned_catalog.binding_tag.clear();
    verify_binding(
        vault,
        ControlDomain::Catalog,
        &catalog_bytes(&unsigned_catalog)?,
        &catalog.binding_tag,
    )?;
    if catalog.vault_id != vault.vault_id().ok_or(KnowledgeStoreError::Corrupt)?
        || anchor.vault_id != catalog.vault_id
        || anchor.catalog_uuid != EvidenceVault::catalog_uuid()
        || catalog.entries.len() > MAX_CATALOG_ENTRIES
        || catalog.generation as usize != catalog.entries.len() + 1
        || metadata.revision != catalog.generation + 1
        || anchor_metadata.revision != 2
    {
        return Err(KnowledgeStoreError::Corrupt);
    }
    let mut identities = BTreeSet::new();
    let mut physical = BTreeSet::new();
    let mut heads: BTreeMap<&str, &RecordRef> = BTreeMap::new();
    for item in &catalog.entries {
        item.reference
            .validate()
            .map_err(|_| KnowledgeStoreError::Corrupt)?;
        unoone_vault_core::vault::validate_record_id(&item.physical_uuid)
            .map_err(|_| KnowledgeStoreError::Corrupt)?;
        if !identities.insert((item.reference.logical_id.clone(), item.reference.revision))
            || !physical.insert(&item.physical_uuid)
            || item.physical_uuid == EvidenceVault::catalog_uuid()
            || item.physical_uuid == EvidenceVault::anchor_uuid()
        {
            return Err(KnowledgeStoreError::Corrupt);
        }
        match heads.get(item.reference.logical_id.as_str()) {
            None if item.reference.revision == 1 && item.previous.is_none() => {}
            Some(previous)
                if item.previous.as_ref() == Some(*previous)
                    && previous.revision.checked_add(1) == Some(item.reference.revision)
                    && allowed_transition(previous.kind, item.reference.kind) => {}
            _ => return Err(KnowledgeStoreError::Corrupt),
        }
        if item.source_id.trim().is_empty()
            || item.source_id.len() > 512
            || item.source_version.trim().is_empty()
            || item.source_version.len() > 256
            || item.source_id.contains('\0')
            || item.source_version.contains('\0')
            || !(item.source_commit.len() == 40 || item.source_commit.len() == 64)
            || !item
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || item.references.len() > MAX_REFERENCES * 3 + 1
            || matches!(
                item.reference.kind,
                KnowledgeKind::Evidence | KnowledgeKind::Invalidation
            ) && item.reference.revision != 1
        {
            return Err(KnowledgeStoreError::Corrupt);
        }
        heads.insert(&item.reference.logical_id, &item.reference);
    }
    // Canonical shape, authenticated publication and backward membership rule
    // out omitted/forged mappings within the trusted-controller boundary. Selected
    // payloads independently reprove these mappings; full audits still read all.
    let mut published = BTreeMap::new();
    for item in &catalog.entries {
        if item
            .previous
            .as_ref()
            .is_some_and(|r| !item.references.contains(r))
        {
            return Err(KnowledgeStoreError::Corrupt);
        }
        for reference in &item.references {
            reference
                .validate()
                .map_err(|_| KnowledgeStoreError::Corrupt)?;
            if published.get(&(reference.logical_id.clone(), reference.revision))
                != Some(&reference)
            {
                return Err(KnowledgeStoreError::Corrupt);
            }
        }
        published.insert(
            (item.reference.logical_id.clone(), item.reference.revision),
            &item.reference,
        );
    }
    Ok((catalog, metadata))
}
fn allowed_transition(previous: KnowledgeKind, next: KnowledgeKind) -> bool {
    use KnowledgeKind::*;
    matches!(
        (previous, next),
        (Candidate, Candidate | VerifiedPattern)
            | (VerifiedPattern, VerifiedPattern | ApprovedProcedure)
            | (ApprovedProcedure, ApprovedProcedure)
    )
}
fn mapping_for(
    record: &KnowledgeRecord,
    physical_uuid: String,
    content_digest: String,
) -> RecordMapping {
    let h = record.header();
    RecordMapping {
        reference: RecordRef {
            logical_id: h.logical_id.clone(),
            revision: h.revision,
            kind: record.kind(),
            content_digest,
        },
        physical_uuid,
        source_id: h.metadata.source_id.clone(),
        source_version: h.metadata.source_version.clone(),
        source_commit: h.metadata.source_commit.clone(),
        previous: h.previous.clone(),
        references: record.references().into_iter().cloned().collect(),
    }
}
pub(crate) fn read_mapping(vault: &Vault, mapping: &RecordMapping) -> Result<StoredKnowledge> {
    let (metadata, bytes) = strict_read(vault, &mapping.physical_uuid)?;
    if bytes.len() > MAX_RECORD_BYTES || metadata.revision != 2 {
        return Err(KnowledgeStoreError::Corrupt);
    }
    check_metadata(&metadata, &mapping.physical_uuid, &bytes)?;
    if digest(&bytes) != mapping.reference.content_digest {
        return Err(KnowledgeStoreError::Corrupt);
    }
    let record = KnowledgeRecord::decode(&bytes).map_err(|_| KnowledgeStoreError::Corrupt)?;
    if record.encode().map_err(|_| KnowledgeStoreError::Corrupt)? != bytes {
        return Err(KnowledgeStoreError::Corrupt);
    }
    if mapping_for(&record, mapping.physical_uuid.clone(), digest(&bytes)) != *mapping {
        return Err(KnowledgeStoreError::Corrupt);
    }
    if let KnowledgeRecord::Evidence(e) = &record {
        if digest(e.content.as_bytes()) != e.content_sha256 {
            return Err(KnowledgeStoreError::Corrupt);
        }
    }
    Ok(StoredKnowledge {
        mapping: mapping.clone(),
        record,
    })
}
pub(crate) type Loaded = BTreeMap<(String, u32), StoredKnowledge>;
pub(crate) fn require_reference<'a>(
    loaded: &'a Loaded,
    reference: &RecordRef,
) -> Result<&'a StoredKnowledge> {
    let record = loaded
        .get(&(reference.logical_id.clone(), reference.revision))
        .ok_or(KnowledgeStoreError::NotFound)?;
    if record.mapping.reference != *reference {
        return Err(KnowledgeStoreError::Corrupt);
    }
    Ok(record)
}
fn validate_edge_semantics(record: &KnowledgeRecord, loaded: &Loaded) -> Result<()> {
    // Classification integrity only, not proof that a check/UI event happened.
    // The Stage 4 runner must attest real checks and explicit UI approval anew.
    let require_evidence_kind = |reference: &RecordRef, kind: EvidenceKind| -> Result<()> {
        match &require_reference(loaded, reference)?.record {
            KnowledgeRecord::Evidence(e) if e.evidence_kind == kind => Ok(()),
            _ => Err(KnowledgeStoreError::InvalidRecord),
        }
    };
    match record {
        KnowledgeRecord::VerifiedPattern(x) => {
            for check in &x.checks {
                require_evidence_kind(check, EvidenceKind::CheckResult)?;
            }
        }
        KnowledgeRecord::ApprovedProcedure(x) => {
            for outcome in &x.outcome_evidence {
                require_evidence_kind(outcome, EvidenceKind::ProcedureRun)?;
            }
            require_evidence_kind(&x.approval_evidence, EvidenceKind::UiApproval)?;
        }
        _ => {}
    }
    for edge in &record.header().edges {
        if edge.relation == EdgeKind::InvalidatedBy {
            match &require_reference(loaded, &edge.target)?.record {
                KnowledgeRecord::Invalidation(x)
                    if x.target.logical_id == record.header().logical_id => {}
                _ => return Err(KnowledgeStoreError::InvalidRecord),
            }
        }
    }
    Ok(())
}
pub(crate) fn load_all(vault: &Vault, catalog: &Catalog) -> Result<Loaded> {
    let mut loaded = Loaded::new();
    for mapping in &catalog.entries {
        let item = read_mapping(vault, mapping)?;
        // Only backward references to already-existing catalog entries are
        // accepted. This validates kinds/hash/identity AND rules out graph cycles.
        for reference in item.record.references() {
            require_reference(&loaded, reference)?;
        }
        validate_edge_semantics(&item.record, &loaded)?;
        loaded.insert(
            (
                mapping.reference.logical_id.clone(),
                mapping.reference.revision,
            ),
            item,
        );
    }
    Ok(loaded)
}
pub(crate) fn require_active(loaded: &Loaded, reference: &RecordRef) -> Result<()> {
    let mut revoked = BTreeSet::new();
    let mut superseded = BTreeSet::new();
    for item in loaded.values() {
        if let KnowledgeRecord::Invalidation(x) = &item.record {
            revoked.insert(x.target.clone());
        }
        for edge in &item.record.header().edges {
            if edge.relation == EdgeKind::Superseding {
                superseded.insert(edge.target.clone());
            }
        }
    }
    // Superseding hides an old root from active retrieval; it does not destroy
    // its provenance or poison the successor that necessarily references it.
    // Explicit invalidation DOES propagate through successor dependencies.
    if superseded.contains(reference) {
        return Err(KnowledgeStoreError::Invalidated);
    }
    let mut seen = BTreeSet::new();
    let mut pending = vec![(reference.clone(), 0usize)];
    while let Some((reference, depth)) = pending.pop() {
        if depth >= MAX_GRAPH_DEPTH || seen.len() >= MAX_CATALOG_ENTRIES {
            return Err(KnowledgeStoreError::Limit);
        }
        if !seen.insert(reference.clone()) {
            continue;
        }
        let item = require_reference(loaded, &reference)?;
        if revoked.contains(&reference)
            || item
                .record
                .header()
                .edges
                .iter()
                .any(|e| e.relation == EdgeKind::InvalidatedBy)
        {
            return Err(KnowledgeStoreError::Invalidated);
        }
        // An invalidation is an audit fact, not dependent on its now-revoked
        // target remaining active. Its references are still validated by load_all.
        if !matches!(item.record, KnowledgeRecord::Invalidation(_)) {
            pending.extend(
                item.record
                    .references()
                    .into_iter()
                    .map(|r| (r.clone(), depth + 1)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const PASSWORD: &[u8] = b"stage2-synthetic-fixture-password";
    fn fixture() -> (tempfile::TempDir, EvidenceVault) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UNOONE");
        Vault::create(&root, PASSWORD).unwrap();
        let mut vault = Vault::open(&root).unwrap();
        vault.unlock(PASSWORD).unwrap();
        (temp, EvidenceVault::new(Arc::new(Mutex::new(Some(vault)))))
    }
    fn header(id: &str) -> RecordHeader {
        RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id: id.into(),
            revision: 1,
            previous: None,
            timestamp_ms: 1,
            audit: Audit {
                actor: "private-actor-marker".into(),
                reason: "private-reason-marker".into(),
            },
            metadata: KnowledgeMetadata {
                source_id: "private-source-marker".into(),
                source_version: "private-version-marker".into(),
                source_commit: "aef047b7b0c0cfb57f1f56ed2be33affcf60247e".into(),
                license: "private-license-marker".into(),
                privacy: KnowledgePrivacy::Private,
                applicability: Applicability {
                    topics: vec!["private-topic-marker".into()],
                    platforms: vec!["private-platform-marker".into()],
                    constraints: "private-path-marker".into(),
                },
            },
            edges: vec![],
        }
    }
    fn evidence(id: &str) -> KnowledgeRecord {
        KnowledgeRecord::Evidence(Evidence {
            header: header(id),
            evidence_kind: EvidenceKind::CheckResult,
            content: "private-payload-marker".into(),
            content_sha256: digest(b"private-payload-marker"),
        })
    }
    fn candidate(id: &str, evidence: &RecordRef) -> KnowledgeRecord {
        KnowledgeRecord::Candidate(Candidate {
            header: header(id),
            statement: "private-statement-marker".into(),
            evidence: vec![evidence.clone()],
        })
    }
    fn assert_no_plaintext(path: &Path) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                assert_no_plaintext(&path);
            } else {
                let raw = std::fs::read(&path).unwrap();
                for marker in [
                    "private-payload-marker",
                    "private-source-marker",
                    "private-license-marker",
                    "private-topic-marker",
                    "private-path-marker",
                    "private-actor-marker",
                    "private-reason-marker",
                    "private-version-marker",
                    "private-platform-marker",
                    "private-statement-marker",
                    "secret-logical-id",
                    "aef047b7b0c0cfb57f1f56ed2be33affcf60247e",
                    "candidate-one",
                    "approved-one",
                    "revoke-one",
                ] {
                    assert!(
                        !raw.windows(marker.len()).any(|w| w == marker.as_bytes()),
                        "plaintext in {}",
                        path.display()
                    );
                    assert!(!path.to_string_lossy().contains(marker));
                }
            }
        }
    }
    fn record_path(store: &EvidenceVault, uuid: &str) -> std::path::PathBuf {
        store
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .vault_root()
            .join("VAULT/records")
            .join(format!("{uuid}.enc.json"))
    }

    fn outcome() -> unoone_capability_contracts::ProcedureOutcome {
        let outcome: unoone_capability_contracts::ProcedureOutcome = serde_json::from_value(serde_json::json!({
            "schema": unoone_capability_contracts::schemas::PROCEDURE,
            "procedure_id": "procedure-one", "bounded_arguments": "fixed",
            "preconditions": "checked", "postconditions": "checked", "result": "success",
            "verification": {"verified": true, "evidence": "recorded-claim-not-authority"}, "risk_class": "LOW",
            "promotion": {"status": "approved", "policy_version": "v1", "requirements": {
                "bounded_arguments": true, "repeatable_success": true, "verified_postconditions": true,
                "low_risk_class": true, "no_contradictory_evidence": true, "explicit_approval": true}},
            "timestamp_ms": 1, "provenance": {"platform": "fixture", "device_id": "fixture", "source": "recorded-claim"}
        })).unwrap();
        outcome
    }

    fn invalidation(id: &str, target: &RecordRef) -> KnowledgeRecord {
        KnowledgeRecord::Invalidation(Invalidation {
            header: header(id),
            target: target.clone(),
            reason: "audited-revocation".into(),
        })
    }

    fn snapshot(path: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        fn visit(path: &Path, files: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, files);
                } else {
                    files.insert(path.clone(), std::fs::read(path).unwrap());
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(path, &mut files);
        files
    }

    #[test]
    fn knowledge_superseded_target_remains_revocable_with_exact_audit_reference() {
        let (temp, store) = fixture();
        store.initialize().unwrap();
        let old = store.create(evidence("superseded-evidence")).unwrap();
        let check = store.create(evidence("independent-check")).unwrap();
        let old_path = record_path(&store, &old.physical_uuid);
        let old_bytes = std::fs::read(&old_path).unwrap();
        let mut successor = candidate("successor", &old.reference);
        if let KnowledgeRecord::Candidate(ref mut x) = successor {
            x.header.edges.push(GraphEdge {
                relation: EdgeKind::Superseding,
                target: old.reference.clone(),
            });
        }
        let successor = store.create(successor).unwrap();
        let mut dependent = candidate("transitive-dependent", &check.reference);
        if let KnowledgeRecord::Candidate(ref mut x) = dependent {
            x.header.edges.push(GraphEdge {
                relation: EdgeKind::Derived,
                target: successor.reference.clone(),
            });
        }
        let dependent = store.create(dependent).unwrap();
        assert!(store.is_invalidated(&old.reference).unwrap());
        assert!(store.read(&old.reference).is_ok());
        assert!(store.read_active(&successor.reference).is_ok());
        assert!(store.read_active(&dependent.reference).is_ok());

        // Nonactive is not missing/tampered. Every rejection is byte-identical,
        // including catalog/journal/other records, not just the target blob.
        for (field, error) in [
            ("digest", KnowledgeStoreError::Corrupt),
            ("kind", KnowledgeStoreError::Corrupt),
            ("revision", KnowledgeStoreError::NotFound),
            ("identity", KnowledgeStoreError::NotFound),
        ] {
            let mut bad = old.reference.clone();
            match field {
                "digest" => bad.content_digest = "a".repeat(64),
                "kind" => bad.kind = KnowledgeKind::Candidate,
                "revision" => bad.revision = 2,
                "identity" => bad.logical_id = "missing-target".into(),
                _ => unreachable!(),
            }
            let before = snapshot(temp.path());
            assert_eq!(
                store.invalidate(invalidation("bad-target", &bad)),
                Err(error)
            );
            assert_eq!(snapshot(temp.path()), before);
        }
        for bytes in [Some(b"tampered-encrypted-target".as_slice()), None] {
            if let Some(bytes) = bytes {
                std::fs::write(&old_path, bytes).unwrap();
            } else {
                std::fs::remove_file(&old_path).unwrap();
            }
            let before = snapshot(temp.path());
            assert!(store
                .invalidate(invalidation("broken-target", &old.reference))
                .is_err());
            assert_eq!(snapshot(temp.path()), before);
            std::fs::write(&old_path, &old_bytes).unwrap();
        }
        let before = snapshot(temp.path());
        // The audit exception is ONLY for the typed target, not extra provenance.
        let mut bad_edge = invalidation("inactive-extra-edge", &check.reference);
        if let KnowledgeRecord::Invalidation(ref mut x) = bad_edge {
            x.header.edges.push(GraphEdge {
                relation: EdgeKind::Derived,
                target: old.reference.clone(),
            });
        }
        assert_eq!(
            store.invalidate(bad_edge),
            Err(KnowledgeStoreError::Invalidated)
        );
        assert_eq!(
            store.create(candidate("inactive-admission", &old.reference)),
            Err(KnowledgeStoreError::Invalidated)
        );
        assert_eq!(snapshot(temp.path()), before);

        let revocation = store
            .invalidate(invalidation("revoke-superseded", &old.reference))
            .unwrap();
        assert!(store.read_active(&revocation.reference).is_ok());
        for reference in [&old.reference, &successor.reference, &dependent.reference] {
            assert_eq!(
                store.read_active(reference),
                Err(KnowledgeStoreError::Invalidated)
            );
            assert!(store.read(reference).is_ok());
        }
        // Explicit policy: another distinct immutable audit fact may revoke an
        // already-revoked exact target; reusing the same audit ID is a conflict.
        let repeated = invalidation("revoke-again", &old.reference);
        let second = store.create(repeated.clone()).unwrap();
        assert!(store.read_active(&second.reference).is_ok());
        let before = snapshot(temp.path());
        assert_eq!(
            store.invalidate(repeated),
            Err(KnowledgeStoreError::Conflict)
        );
        assert_eq!(snapshot(temp.path()), before);
        assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
        assert_eq!(store.graph_edges(&old.reference).unwrap().len(), 2);
    }

    #[test]
    fn knowledge_immutable_invalidation_rejects_superseding_and_all_kind_transitions() {
        let (temp, store) = fixture();
        store.initialize().unwrap();
        let old = store.create(evidence("revoked-evidence")).unwrap();
        let revocation = store
            .invalidate(invalidation("immutable-revocation", &old.reference))
            .unwrap();
        let revocation_bytes =
            std::fs::read(record_path(&store, &revocation.physical_uuid)).unwrap();
        let before = snapshot(temp.path());
        let mut hostile = evidence("supersede-invalidation");
        if let KnowledgeRecord::Evidence(ref mut x) = hostile {
            x.header.edges.push(GraphEdge {
                relation: EdgeKind::Superseding,
                target: revocation.reference.clone(),
            });
        }
        assert_eq!(
            store.create(hostile),
            Err(KnowledgeStoreError::InvalidRecord)
        );
        let mut transition_header = header("immutable-revocation");
        transition_header.revision = 2;
        transition_header.previous = Some(revocation.reference.clone());
        let transitions = [
            KnowledgeRecord::Evidence(Evidence {
                header: transition_header.clone(),
                evidence_kind: EvidenceKind::Observation,
                content: "fixture".into(),
                content_sha256: digest(b"fixture"),
            }),
            KnowledgeRecord::Candidate(Candidate {
                header: transition_header.clone(),
                statement: "fixture".into(),
                evidence: vec![old.reference.clone()],
            }),
            KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                header: transition_header.clone(),
                candidate: RecordRef {
                    kind: KnowledgeKind::Candidate,
                    ..old.reference.clone()
                },
                checks: vec![old.reference.clone()],
                statement: "fixture".into(),
            }),
            KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
                header: transition_header.clone(),
                pattern: RecordRef {
                    kind: KnowledgeKind::VerifiedPattern,
                    ..old.reference.clone()
                },
                outcome: outcome(),
                outcome_evidence: vec![old.reference.clone()],
                approval_evidence: old.reference.clone(),
            })),
            KnowledgeRecord::Invalidation(Invalidation {
                header: transition_header,
                target: old.reference.clone(),
                reason: "fixture".into(),
            }),
        ];
        for record in transitions {
            assert!(!allowed_transition(
                KnowledgeKind::Invalidation,
                record.kind()
            ));
            assert!(store
                .append_revision(record, &revocation.reference)
                .is_err());
        }
        assert!(store
            .invalidate(invalidation("revoke-revocation", &revocation.reference))
            .is_err());
        assert_eq!(snapshot(temp.path()), before);
        assert!(store.read_active(&revocation.reference).is_ok());
        assert!(!store.is_invalidated(&revocation.reference).unwrap());
        assert!(store.is_invalidated(&old.reference).unwrap());
        assert_eq!(
            std::fs::read(record_path(&store, &revocation.physical_uuid)).unwrap(),
            revocation_bytes
        );
        // Reopening the actual encrypted vault retains the same audit semantics.
        let root = temp.path().join("UNOONE");
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .lock()
            .unwrap();
        let mut reopened = Vault::open(&root).unwrap();
        reopened.unlock(PASSWORD).unwrap();
        *store.vault.lock().unwrap() = Some(reopened);
        assert!(store.read_active(&revocation.reference).is_ok());
        assert!(store.is_invalidated(&old.reference).unwrap());
    }

    #[test]
    fn knowledge_real_vault_roundtrip_revisions_revoke_lock_and_absence() {
        let (temp, store) = fixture();
        assert!(matches!(
            store.catalog(),
            Err(KnowledgeStoreError::Uninitialized)
        ));
        store.initialize().unwrap();
        assert!(store.catalog().unwrap().is_empty());
        let catalog_path = record_path(&store, &EvidenceVault::catalog_uuid());
        let empty_catalog = std::fs::read(&catalog_path).unwrap();
        store.initialize().unwrap();
        assert_eq!(std::fs::read(&catalog_path).unwrap(), empty_catalog);
        assert_ne!(EvidenceVault::catalog_uuid(), EvidenceVault::anchor_uuid());
        for scope in [
            "conversation",
            "preferences",
            "relevant",
            "project",
            "document",
            "extended",
        ] {
            let namespace = format!(
                "pai-harness-memory-v1\0{scope}\0__pai_harness_internal__\0memory-index-v1"
            );
            assert_ne!(
                EvidenceVault::catalog_uuid(),
                namespaced_uuid(namespace.as_bytes())
            );
            assert_ne!(
                EvidenceVault::anchor_uuid(),
                namespaced_uuid(namespace.as_bytes())
            );
        }
        let mut bad_evidence = evidence("bad-content-hash");
        if let KnowledgeRecord::Evidence(ref mut x) = bad_evidence {
            x.content_sha256 = "a".repeat(64);
        }
        assert!(store.create(bad_evidence).is_err());
        let e = store.create(evidence("secret-logical-id")).unwrap();
        assert_eq!(store.read(&e.reference).unwrap().mapping, e);
        let original = std::fs::read(record_path(&store, &e.physical_uuid)).unwrap();
        assert!(matches!(
            store.create(evidence("secret-logical-id")),
            Err(KnowledgeStoreError::Conflict)
        ));
        assert_eq!(
            std::fs::read(record_path(&store, &e.physical_uuid)).unwrap(),
            original
        );
        let first = store
            .create(candidate("candidate-one", &e.reference))
            .unwrap();
        let mut next = candidate("candidate-one", &e.reference);
        if let KnowledgeRecord::Candidate(ref mut c) = next {
            c.header.revision = 2;
            c.header.previous = Some(first.reference.clone());
            c.header.audit.reason = "revision-two".into();
        }
        let second = store
            .append_revision(next.clone(), &first.reference)
            .unwrap();
        assert!(matches!(
            store.append_revision(next, &first.reference),
            Err(KnowledgeStoreError::Conflict)
        ));
        assert_eq!(store.history("candidate-one").unwrap().len(), 2);
        assert_eq!(store.read_latest("candidate-one").unwrap().mapping, second);
        let pattern = store
            .create(KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                header: header("pattern-one"),
                candidate: second.reference.clone(),
                checks: vec![e.reference.clone()],
                statement: "verified-metadata".into(),
            }))
            .unwrap();
        assert!(!store
            .read_active(&pattern.reference)
            .unwrap()
            .record
            .authorizes_execution());
        // Two store instances sharing the product vault mutex cannot both win
        // the same optimistic revision, even when raced from different threads.
        let race = store
            .create(candidate("race-candidate", &e.reference))
            .unwrap();
        let mut race_next = candidate("race-candidate", &e.reference);
        if let KnowledgeRecord::Candidate(ref mut c) = race_next {
            c.header.revision = 2;
            c.header.previous = Some(race.reference.clone());
        }
        let other = EvidenceVault::new(store.vault.clone());
        let expected = race.reference.clone();
        let other_next = race_next.clone();
        let worker = std::thread::spawn(move || other.append_revision(other_next, &expected));
        let local = store.append_revision(race_next, &race.reference);
        let remote = worker.join().unwrap();
        assert_eq!(usize::from(local.is_ok()) + usize::from(remote.is_ok()), 1);
        assert!(
            matches!(local, Err(KnowledgeStoreError::Conflict))
                || matches!(remote, Err(KnowledgeStoreError::Conflict))
        );
        assert_eq!(store.history("race-candidate").unwrap().len(), 2);
        let mut successor = candidate("successor-one", &e.reference);
        if let KnowledgeRecord::Candidate(ref mut c) = successor {
            c.header.edges = vec![
                GraphEdge {
                    relation: EdgeKind::Supporting,
                    target: e.reference.clone(),
                },
                GraphEdge {
                    relation: EdgeKind::Contradicting,
                    target: first.reference.clone(),
                },
                GraphEdge {
                    relation: EdgeKind::Derived,
                    target: second.reference.clone(),
                },
                GraphEdge {
                    relation: EdgeKind::Superseding,
                    target: race.reference.clone(),
                },
            ];
        }
        let successor = store.create(successor).unwrap();
        assert!(store.read_active(&successor.reference).is_ok());
        assert!(store.is_invalidated(&race.reference).unwrap());
        assert!(store.read(&race.reference).is_ok());
        let mut run = evidence("run-one");
        if let KnowledgeRecord::Evidence(ref mut x) = run {
            x.evidence_kind = EvidenceKind::ProcedureRun;
        }
        let run = store.create(run).unwrap();
        let mut approval = evidence("ui-approval-one");
        if let KnowledgeRecord::Evidence(ref mut x) = approval {
            x.evidence_kind = EvidenceKind::UiApproval;
        }
        let approval = store.create(approval).unwrap();
        let outcome = outcome();
        let mut approved = KnowledgeRecord::ApprovedProcedure(Box::new(ApprovedProcedure {
            header: header("approved-one"),
            pattern: pattern.reference.clone(),
            outcome,
            outcome_evidence: vec![run.reference.clone()],
            approval_evidence: e.reference.clone(),
        }));
        assert!(store.create(approved.clone()).is_err()); // a check-result is NOT an approval event
        if let KnowledgeRecord::ApprovedProcedure(ref mut p) = approved {
            p.approval_evidence = approval.reference.clone();
        }
        let approved = store.create(approved).unwrap();
        assert!(!store
            .read_active(&approved.reference)
            .unwrap()
            .record
            .authorizes_execution());
        // The same logical lineage can append Candidate -> VerifiedPattern ->
        // ApprovedProcedure metadata without rewriting any predecessor blob.
        let workflow = store
            .create(candidate("workflow-one", &e.reference))
            .unwrap();
        let mut workflow_header = header("workflow-one");
        workflow_header.revision = 2;
        workflow_header.previous = Some(workflow.reference.clone());
        let workflow_pattern = store
            .append_revision(
                KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                    header: workflow_header,
                    candidate: workflow.reference.clone(),
                    checks: vec![e.reference.clone()],
                    statement: "workflow-verification-metadata".into(),
                }),
                &workflow.reference,
            )
            .unwrap();
        let approved_record = store.read(&approved.reference).unwrap().record;
        let mut workflow_approved = match approved_record {
            KnowledgeRecord::ApprovedProcedure(p) => p,
            _ => unreachable!(),
        };
        workflow_approved.header = header("workflow-one");
        workflow_approved.header.revision = 3;
        workflow_approved.header.previous = Some(workflow_pattern.reference.clone());
        workflow_approved.pattern = workflow_pattern.reference.clone();
        let workflow_approved = store
            .append_revision(
                KnowledgeRecord::ApprovedProcedure(workflow_approved),
                &workflow_pattern.reference,
            )
            .unwrap();
        assert_eq!(store.history("workflow-one").unwrap().len(), 3);
        assert!(!store
            .read_active(&workflow_approved.reference)
            .unwrap()
            .record
            .authorizes_execution());
        let invalidation = KnowledgeRecord::Invalidation(Invalidation {
            header: header("revoke-one"),
            target: e.reference.clone(),
            reason: "revoked-fixture".into(),
        });
        let revocation = store.invalidate(invalidation).unwrap();
        assert!(store.read_active(&revocation.reference).is_ok());
        assert_eq!(
            store.graph_edges(&e.reference).unwrap(),
            vec![GraphEdge {
                relation: EdgeKind::InvalidatedBy,
                target: revocation.reference.clone()
            }]
        );
        assert!(matches!(
            store.read_active(&approved.reference),
            Err(KnowledgeStoreError::Invalidated)
        ));
        assert!(matches!(
            store.read_active(&successor.reference),
            Err(KnowledgeStoreError::Invalidated)
        ));
        assert!(store.read(&e.reference).is_ok()); // immutable audit remains readable
        assert!(matches!(
            store.read_active(&e.reference),
            Err(KnowledgeStoreError::Invalidated)
        ));
        assert!(matches!(
            store.read_active(&pattern.reference),
            Err(KnowledgeStoreError::Invalidated)
        ));
        assert!(store
            .create(candidate("after-revoke", &e.reference))
            .is_err());
        assert_no_plaintext(temp.path());
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .lock()
            .unwrap();
        assert!(matches!(store.catalog(), Err(KnowledgeStoreError::Locked)));
        assert!(store.create(evidence("locked-write")).is_err());
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .unlock(PASSWORD)
            .unwrap();
        assert!(store.read(&first.reference).is_ok());
        let root = store
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .vault_root()
            .to_path_buf();
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .lock()
            .unwrap();
        let mut reopened = Vault::open(&root).unwrap();
        reopened.unlock(PASSWORD).unwrap();
        *store.vault.lock().unwrap() = Some(reopened);
        assert_eq!(store.history("candidate-one").unwrap().len(), 2);
        *store.vault.lock().unwrap() = None;
        assert!(matches!(store.catalog(), Err(KnowledgeStoreError::Locked)));
    }

    #[test]
    fn knowledge_real_vault_foreign_schema_corruption_missing_refs_and_collision() {
        let (_temp, store) = fixture();
        // Occupied fixed UUID must never be silently replaced, even in an otherwise empty vault.
        let uuid = EvidenceVault::catalog_uuid();
        let mut foreign = Record::new(RecordType::Memory, "fixture", "fixture");
        foreign.record_id = uuid.clone();
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .write_record(foreign, b"foreign-private-data")
            .unwrap();
        let path = record_path(&store, &uuid);
        let bytes = std::fs::read(&path).unwrap();
        assert!(store.initialize().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        // Only test cleanup removes the foreign blob; production never deletes or replaces it.
        std::fs::remove_file(&path).unwrap();
        store.initialize().unwrap();
        let e = store.create(evidence("evidence-one")).unwrap();
        let mut bad = e.reference.clone();
        bad.content_digest = "a".repeat(64);
        assert!(store.create(candidate("bad-hash", &bad)).is_err());
        bad = e.reference.clone();
        bad.kind = KnowledgeKind::Candidate;
        assert!(store.read(&bad).is_err());
        bad = e.reference.clone();
        bad.logical_id = "missing-ref".into();
        assert!(store
            .create(candidate("missing-ref-candidate", &bad))
            .is_err());
        let p = record_path(&store, &e.physical_uuid);
        let saved = std::fs::read(&p).unwrap();
        std::fs::write(&p, b"corrupted-blob").unwrap();
        assert!(store.read(&e.reference).is_err());
        assert!(store
            .create(candidate("corrupt-ref", &e.reference))
            .is_err());
        std::fs::write(&p, &saved).unwrap();
        let mut ciphertext: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        let mut encrypted = ciphertext["encrypted_content"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec();
        encrypted[0] = if encrypted[0] == b'a' { b'b' } else { b'a' };
        ciphertext["encrypted_content"] = String::from_utf8(encrypted).unwrap().into();
        std::fs::write(&p, serde_json::to_vec(&ciphertext).unwrap()).unwrap();
        assert!(store.read(&e.reference).is_err());
        assert!(store
            .create(candidate("aead-tampered-ref", &e.reference))
            .is_err());
        std::fs::write(&p, &saved).unwrap();
        // Authenticated payload hash mismatch is rejected too (not just malformed encrypted JSON).
        let (metadata, mut payload) = store
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .read_record(&e.physical_uuid)
            .unwrap();
        payload.push(b' ');
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .write_record(metadata, &payload)
            .unwrap();
        assert!(store.read(&e.reference).is_err());
        std::fs::write(&p, &saved).unwrap();
        let catalog_saved = std::fs::read(&path).unwrap();
        let (metadata, payload) = store
            .vault
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .read_record(&uuid)
            .unwrap();
        for version in [0, 99] {
            let mut json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            json["schema_version"] = version.into();
            store
                .vault
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .write_record(metadata.clone(), &serde_json::to_vec(&json).unwrap())
                .unwrap();
            let foreign_bytes = std::fs::read(&path).unwrap();
            assert!(store.catalog().is_err());
            assert!(store.initialize().is_err());
            assert!(store.create(evidence("cannot-replace-schema")).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), foreign_bytes);
        }
        std::fs::write(&path, &catalog_saved).unwrap();
        // Unknown catalog fields, broken mappings and legacy-AAD downgrades fail
        // closed without writeback, even when ciphertext was validly authored.
        let mut foreign_catalog: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        foreign_catalog["extra_authority"] = true.into();
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .write_record(
                metadata.clone(),
                &serde_json::to_vec(&foreign_catalog).unwrap(),
            )
            .unwrap();
        assert!(store.catalog().is_err());
        std::fs::write(&path, &catalog_saved).unwrap();
        for aad_version in [0, 3] {
            let mut json: serde_json::Value = serde_json::from_slice(&catalog_saved).unwrap();
            json["aad_version"] = aad_version.into();
            std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
            assert!(store.catalog().is_err());
            assert!(store.initialize().is_err());
        }
        std::fs::write(&path, &catalog_saved).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(store.catalog(), Err(KnowledgeStoreError::Corrupt)));
        assert!(store.initialize().is_err()); // anchor prevents resurrection
        std::fs::write(&path, &catalog_saved).unwrap();
        // Tamper with plaintext metadata; canonical AEAD verification must fail.
        let mut json: serde_json::Value = serde_json::from_slice(&catalog_saved).unwrap();
        json["metadata"]["origin_device_id"] = "tampered".into();
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(store.catalog().is_err());
        std::fs::write(&path, &catalog_saved).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert!(store.read(&e.reference).is_err());
        assert!(store
            .create(candidate("deleted-ref", &e.reference))
            .is_err());
        std::fs::write(&p, &saved).unwrap();
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .delete_record(&e.physical_uuid, "fixture", "fixture")
            .unwrap();
        assert!(store.read(&e.reference).is_err());
        assert!(store
            .create(candidate("tombstoned-ref", &e.reference))
            .is_err());
        store
            .vault
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .delete_record(&uuid, "fixture", "fixture")
            .unwrap();
        assert!(store.initialize().is_err());
        assert!(store.catalog().is_err());
    }
}
