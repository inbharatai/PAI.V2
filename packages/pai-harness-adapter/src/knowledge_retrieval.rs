//! Stage 3 lexical/symbol retrieval, NOT embedding semantics or execution authority.
//! Postings, scope and identifiers are encrypted in the SAME shared Vault. Searches
//! are read-only, hold the vault mutex throughout and retain no decrypted cache.

use crate::knowledge::{self, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use unoone_capability_contracts::knowledge::*;
use unoone_vault_core::{Record, Vault};

pub const INDEX_SCHEMA: &str = "inbharat.pai.knowledge.postings";
pub const INDEX_VERSION: u32 = 2;
pub const FENCE_SCHEMA: &str = "inbharat.pai.knowledge.applicability.v1";
const MAX_TERMS: usize = 65536;
const MAX_POSTINGS: usize = 262144;
const MAX_DOCUMENT_TERMS: usize = 512;
const MAX_QUERY_CHARS: usize = 256;
const MAX_QUERY_TERMS: usize = 16;

/// Structured Stage3 constraint convention INSIDE the existing v1 constraints
/// string. Old free-text constraints remain byte-compatible, historical-only.
/// This digest identifies the source file, not the knowledge-record digest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApplicabilityFence {
    pub schema: String,
    pub file_digest: String,
}
impl ApplicabilityFence {
    pub fn encode(&self) -> std::result::Result<String, RetrievalError> {
        if self.schema != FENCE_SCHEMA || !valid_digest(&self.file_digest) {
            return Err(RetrievalError::InvalidQuery);
        }
        serde_json::to_string(self).map_err(|_| RetrievalError::InvalidQuery)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecallMode {
    Current,
    Historical,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Terms,
    Symbol,
}
/// Trust comes from the caller's independently established source identity, NOT
/// a model, index or user profile. Supplying strings is not source attestation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustedSource {
    pub source_id: String,
    pub source_version: String,
    pub source_commit: String,
    pub file_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetrievalQuery {
    pub text: String,
    pub kind: QueryKind,
    pub mode: RecallMode,
    pub platform: String,
    pub trusted_source: Option<TrustedSource>,
    pub topic: Option<String>,
    pub candidate_limit: usize,
    pub result_limit: usize,
    /// Unicode scalar count of serialized hits (source/provenance/why included).
    pub context_chars: usize,
    pub snippet_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalError {
    Store(KnowledgeStoreError),
    UninitializedIndex,
    StaleIndex,
    UnsupportedIndexSchema,
    InvalidQuery,
    MissingTrustedVersion,
    IndexLimit,
}
impl From<KnowledgeStoreError> for RetrievalError {
    fn from(value: KnowledgeStoreError) -> Self {
        Self::Store(value)
    }
}
impl std::fmt::Display for RetrievalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "knowledge retrieval: {self:?}")
    }
}
impl std::error::Error for RetrievalError {}
type Result<T> = std::result::Result<T, RetrievalError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Document {
    reference: RecordRef,
    source: TrustedSource,
    platforms: Vec<String>,
    topics: Vec<String>,
    /// Missing structured digest admits historical metadata only.
    has_fence: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Posting {
    document: usize,
    weight: u16,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PostingsIndex {
    schema: String,
    schema_version: u32,
    vault_id: String,
    revision: u32,
    catalog_generation: u32,
    catalog_digest: String,
    documents: Vec<Document>,
    postings: BTreeMap<String, Vec<Posting>>,
    #[serde(default)]
    binding_tag: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexAnchor {
    schema: String,
    schema_version: u32,
    vault_id: String,
    index_uuid: String,
    #[serde(default)]
    binding_tag: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexBuildReport {
    pub catalog_generation: u32,
    pub documents: usize,
    pub terms: usize,
    pub postings: usize,
    pub payload_decryptions: usize,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NormalizationAudit {
    pub algorithm: String,
    pub terms: Vec<String>,
    pub declared_aliases_used: Vec<String>,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecallHit {
    pub reference: RecordRef,
    pub source: TrustedSource,
    pub platforms: Vec<String>,
    pub topics: Vec<String>,
    pub provenance: Vec<RecordRef>,
    pub snippet: String,
    pub score: u32,
    pub why_recalled: String,
    pub active: bool,
    pub contradictory: bool,
    /// Historical is audit-only, including stale/revoked/contradictory material.
    pub mode: RecallMode,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RetrievalStats {
    pub catalog_entries: usize,
    pub posting_candidates: usize,
    pub selected_candidates: usize,
    pub payload_decryptions: usize,
    /// Catalog, catalog anchor, index and index anchor are decrypted separately.
    pub control_decryptions: usize,
    pub context_chars: usize,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecallResult {
    pub catalog_generation: u32,
    pub normalization: NormalizationAudit,
    pub hits: Vec<RecallHit>,
    pub stats: RetrievalStats,
}

pub struct KnowledgeRetriever {
    store: EvidenceVault,
}
impl KnowledgeRetriever {
    pub fn new(vault: Arc<Mutex<Option<Vault>>>) -> Self {
        Self {
            store: EvidenceVault::new(vault),
        }
    }
    pub fn index_uuid() -> String {
        knowledge::namespaced_uuid(b"inbharat.pai.knowledge.postings.v1\0")
    }
    pub fn anchor_uuid() -> String {
        knowledge::namespaced_uuid(b"inbharat.pai.knowledge.postings-anchor.v1\0")
    }
    /// Explicit full-validation rebuild. Never repairs occupied foreign/corrupt,
    /// tombstoned or partial index/anchor contents, even when the catalog is stale.
    /// A catalog mutation never silently writes the index during a search.
    pub fn rebuild(&self) -> Result<IndexBuildReport> {
        let mut outcome = None;
        self.store.with_vault(|vault| {
            let (catalog, metadata) = knowledge::load_catalog(vault)?;
            let loaded = knowledge::load_all(vault, &catalog)?;
            let old = match load_index(vault) {
                Ok(value) => Some(value),
                Err(RetrievalError::UninitializedIndex) => None,
                Err(e) => {
                    outcome = Some(Err(e));
                    return Ok(());
                }
            };
            // An occupied, authenticated index must be an exact derivative of
            // the immutable catalog prefix it claims, not merely valid JSON.
            // Explicit rebuilding authenticates all payloads anyway; recompute
            // that prefix before overwriting even a stale index.
            if let Some((previous, _)) = &old {
                if previous.documents.len() > catalog.entries.len() {
                    return Err(KnowledgeStoreError::Corrupt);
                }
                let mut prefix = catalog.clone();
                prefix.entries.truncate(previous.documents.len());
                prefix.generation = previous.catalog_generation;
                knowledge::seal_catalog(vault, &mut prefix)?;
                let prefix_digest = knowledge::digest(&knowledge::catalog_bytes(&prefix)?);
                let mut expected =
                    match build_index(&prefix, &prefix_digest, &loaded, previous.revision) {
                        Ok(index) => index,
                        Err(e) => {
                            outcome = Some(Err(e));
                            return Ok(());
                        }
                    };
                seal_index(vault, &mut expected)?;
                if *previous != expected {
                    return Err(KnowledgeStoreError::Corrupt);
                }
            }
            let revision = match &old {
                Some((_, record)) => record
                    .revision
                    .checked_add(1)
                    .ok_or(KnowledgeStoreError::Limit)?,
                None => 2,
            };
            let mut index = match build_index(&catalog, &metadata.content_hash, &loaded, revision) {
                Ok(index) => index,
                Err(e) => {
                    outcome = Some(Err(e));
                    return Ok(());
                }
            };
            // Preflight the final tag size without signing invalid metadata.
            index.binding_tag = "0".repeat(64);
            let bytes = knowledge::control_bytes(&index)?;
            if bytes.len() > MAX_CATALOG_BYTES {
                outcome = Some(Err(RetrievalError::IndexLimit));
                return Ok(());
            }
            if let Err(e) = validate_index(
                &index,
                vault.vault_id().ok_or(KnowledgeStoreError::Corrupt)?,
                revision,
            ) {
                outcome = Some(Err(e));
                return Ok(());
            }
            validate_binding(&index, &catalog)?;
            seal_index(vault, &mut index)?;
            let bytes = knowledge::control_bytes(&index)?;
            match old {
                None => {
                    let mut anchor = IndexAnchor {
                        schema: format!("{INDEX_SCHEMA}.anchor"),
                        schema_version: INDEX_VERSION,
                        vault_id: catalog.vault_id.clone(),
                        index_uuid: Self::index_uuid(),
                        binding_tag: String::new(),
                    };
                    let bytes_anchor =
                        serde_json::to_vec(&anchor).map_err(|_| KnowledgeStoreError::Corrupt)?;
                    if bytes_anchor.len() > 1024 {
                        return Err(KnowledgeStoreError::Limit);
                    }
                    if let Err(e) = validate_anchor(&anchor, &catalog.vault_id, 2) {
                        outcome = Some(Err(e));
                        return Ok(());
                    }
                    anchor.binding_tag = knowledge::sign_binding(
                        vault,
                        ControlDomain::IndexAnchor,
                        &knowledge::control_bytes(&anchor)?,
                    )?;
                    let bytes_anchor = knowledge::control_bytes(&anchor)?;
                    if bytes_anchor.len() > 1024 {
                        return Err(KnowledgeStoreError::Limit);
                    }
                    knowledge::write_new(vault, &Self::anchor_uuid(), &bytes_anchor)?;
                    knowledge::write_new(vault, &Self::index_uuid(), &bytes)?;
                }
                Some((_, metadata)) => {
                    vault
                        .write_record(metadata, &bytes)
                        .map_err(knowledge::map_error)?;
                }
            }
            outcome = Some(load_index(vault).map(|(committed, _)| IndexBuildReport {
                catalog_generation: committed.catalog_generation,
                documents: committed.documents.len(),
                terms: committed.postings.len(),
                postings: committed.postings.values().map(Vec::len).sum(),
                payload_decryptions: loaded.len(),
            }));
            Ok(())
        })?;
        outcome.ok_or(KnowledgeStoreError::Corrupt)?
    }
    pub fn search(&self, query: &RetrievalQuery) -> Result<RecallResult> {
        let normalization = normalize_query(query)?;
        // Entire selection and reads share the same lock: lock/revocation/mutation
        // cannot intervene between catalog binding, candidate IDs and blob reads.
        let mut outcome = None;
        self.store.with_vault(|vault| {
            let (catalog, metadata) = knowledge::load_catalog(vault)?;
            let (index, _) = match load_index(vault) {
                Ok(value) => value,
                Err(e) => {
                    outcome = Some(Err(e));
                    return Ok(());
                }
            };
            if index.catalog_generation != catalog.generation
                || index.catalog_digest != metadata.content_hash
            {
                outcome = Some(Err(RetrievalError::StaleIndex));
                return Ok(());
            }
            validate_binding(&index, &catalog)?;
            let (candidates, total) = candidates(&index, query, &normalization);
            let refs: Vec<_> = candidates
                .iter()
                .map(|(i, _)| index.documents[*i].reference.clone())
                .collect();
            let selected = knowledge::read_selected(vault, &catalog, &refs)?;
            // Scope AND every selected document's exact lexical term/weight
            // derivative must match its authenticated payload, not just AEAD-valid
            // index JSON. Check all selected features, including non-query terms,
            // before current/historical rendering or trusting the supplied score.
            for (item, (i, _)) in selected.items.iter().zip(&candidates) {
                if document(&item.stored.record, &item.stored.mapping.reference)
                    != index.documents[*i]
                {
                    return Err(KnowledgeStoreError::Corrupt);
                }
                let actual = lexical_weights(&item.stored.record)
                    .map_err(|_| KnowledgeStoreError::Corrupt)?;
                let claimed: BTreeMap<_, _> = index
                    .postings
                    .iter()
                    .filter_map(|(term, list)| {
                        list.binary_search_by_key(i, |p| p.document)
                            .ok()
                            .map(|position| (term.clone(), list[position].weight))
                    })
                    .collect();
                if actual != claimed {
                    return Err(KnowledgeStoreError::Corrupt);
                }
            }
            outcome = Some(render_hits(
                &index,
                query,
                normalization,
                candidates,
                total,
                selected,
            ));
            Ok(())
        })?;
        outcome.ok_or(KnowledgeStoreError::Corrupt)?
    }
}

fn seal_index(vault: &Vault, index: &mut PostingsIndex) -> knowledge::Result<()> {
    index.binding_tag.clear();
    index.binding_tag = knowledge::sign_binding(
        vault,
        ControlDomain::Index,
        &knowledge::control_bytes(index)?,
    )?;
    Ok(())
}
fn load_index(vault: &Vault) -> Result<(PostingsIndex, Record)> {
    let raw = knowledge::optional_record(vault, &KnowledgeRetriever::index_uuid())?;
    let anchor = knowledge::optional_record(vault, &KnowledgeRetriever::anchor_uuid())?;
    let ((metadata, bytes), (anchor_metadata, anchor_bytes)) = match (raw, anchor) {
        (None, None) => return Err(RetrievalError::UninitializedIndex),
        (Some(i), Some(a)) => (i, a),
        _ => return Err(KnowledgeStoreError::Corrupt.into()),
    };
    if bytes.len() > MAX_CATALOG_BYTES || anchor_bytes.len() > 1024 {
        return Err(RetrievalError::IndexLimit);
    }
    knowledge::check_metadata(&metadata, &KnowledgeRetriever::index_uuid(), &bytes)?;
    knowledge::check_metadata(
        &anchor_metadata,
        &KnowledgeRetriever::anchor_uuid(),
        &anchor_bytes,
    )?;
    let index: PostingsIndex =
        serde_json::from_slice(&bytes).map_err(|_| KnowledgeStoreError::Corrupt)?;
    let anchor: IndexAnchor =
        serde_json::from_slice(&anchor_bytes).map_err(|_| KnowledgeStoreError::Corrupt)?;
    let vault_id = vault.vault_id().ok_or(KnowledgeStoreError::Corrupt)?;
    validate_anchor(&anchor, vault_id, anchor_metadata.revision)?;
    validate_index(&index, vault_id, metadata.revision)?;
    if knowledge::control_bytes(&index)? != bytes
        || knowledge::control_bytes(&anchor)? != anchor_bytes
    {
        return Err(KnowledgeStoreError::Corrupt.into());
    }
    let mut unsigned_index = index.clone();
    unsigned_index.binding_tag.clear();
    knowledge::verify_binding(
        vault,
        ControlDomain::Index,
        &knowledge::control_bytes(&unsigned_index)?,
        &index.binding_tag,
    )?;
    let mut unsigned_anchor = anchor.clone();
    unsigned_anchor.binding_tag.clear();
    knowledge::verify_binding(
        vault,
        ControlDomain::IndexAnchor,
        &knowledge::control_bytes(&unsigned_anchor)?,
        &anchor.binding_tag,
    )?;
    Ok((index, metadata))
}
fn validate_anchor(anchor: &IndexAnchor, vault_id: &str, revision: u32) -> Result<()> {
    if anchor.schema != format!("{INDEX_SCHEMA}.anchor") || anchor.schema_version != INDEX_VERSION {
        return Err(RetrievalError::UnsupportedIndexSchema);
    }
    if anchor.vault_id != vault_id
        || anchor.index_uuid != KnowledgeRetriever::index_uuid()
        || revision != 2
    {
        return Err(KnowledgeStoreError::Corrupt.into());
    }
    Ok(())
}
/// Shared candidate/read validator: no index can be published that its reader
/// would reject. This validation is complete before ANY index or anchor write.
fn validate_index(index: &PostingsIndex, vault_id: &str, revision: u32) -> Result<()> {
    if index.schema != INDEX_SCHEMA || index.schema_version != INDEX_VERSION {
        return Err(RetrievalError::UnsupportedIndexSchema);
    }
    if index.vault_id != vault_id
        || index.revision != revision
        || index.revision < 2
        || index.catalog_generation as usize != index.documents.len() + 1
        || !valid_digest(&index.catalog_digest)
        || index.documents.len() > MAX_CATALOG_ENTRIES
        || index.postings.len() > MAX_TERMS
    {
        return Err(KnowledgeStoreError::Corrupt.into());
    }
    let mut ids = BTreeSet::new();
    for d in &index.documents {
        d.reference
            .validate()
            .map_err(|_| KnowledgeStoreError::Corrupt)?;
        if !ids.insert(d.reference.clone())
            || !valid_source(&d.source, d.has_fence)
            || !valid_strings(&d.platforms, 16, 128)
            || !valid_strings(&d.topics, 32, 256)
        {
            return Err(KnowledgeStoreError::Corrupt.into());
        }
    }
    let mut count = 0;
    for (term, list) in &index.postings {
        if term.is_empty()
            || term.chars().count() > MAX_QUERY_CHARS
            || lexical_terms(term).as_slice() != [term.clone()]
            || list.is_empty()
        {
            return Err(KnowledgeStoreError::Corrupt.into());
        }
        let mut previous = None;
        for p in list {
            if p.document >= index.documents.len()
                || p.weight == 0
                || p.weight > 64
                || previous.is_some_and(|i| i >= p.document)
            {
                return Err(KnowledgeStoreError::Corrupt.into());
            }
            previous = Some(p.document);
            count += 1;
        }
    }
    if count > MAX_POSTINGS {
        return Err(RetrievalError::IndexLimit);
    }
    Ok(())
}
fn validate_binding(index: &PostingsIndex, catalog: &knowledge::Catalog) -> knowledge::Result<()> {
    if index.documents.len() != catalog.entries.len() {
        return Err(KnowledgeStoreError::Corrupt);
    }
    for (d, m) in index.documents.iter().zip(&catalog.entries) {
        if d.reference != m.reference
            || d.source.source_id != m.source_id
            || d.source.source_version != m.source_version
            || d.source.source_commit != m.source_commit
            || m.physical_uuid == KnowledgeRetriever::index_uuid()
            || m.physical_uuid == KnowledgeRetriever::anchor_uuid()
        {
            return Err(KnowledgeStoreError::Corrupt);
        }
    }
    Ok(())
}
fn document(record: &KnowledgeRecord, reference: &RecordRef) -> Document {
    let m = &record.header().metadata;
    let fence = serde_json::from_str::<ApplicabilityFence>(&m.applicability.constraints)
        .ok()
        .filter(|f| f.schema == FENCE_SCHEMA && valid_digest(&f.file_digest));
    Document {
        reference: reference.clone(),
        source: TrustedSource {
            source_id: m.source_id.clone(),
            source_version: m.source_version.clone(),
            source_commit: m.source_commit.clone(),
            file_digest: fence
                .as_ref()
                .map_or_else(String::new, |f| f.file_digest.clone()),
        },
        platforms: m.applicability.platforms.clone(),
        topics: m.applicability.topics.clone(),
        has_fence: fence.is_some(),
    }
}
fn body(record: &KnowledgeRecord) -> &str {
    match record {
        KnowledgeRecord::Evidence(x) => &x.content,
        KnowledgeRecord::Candidate(x) => &x.statement,
        KnowledgeRecord::VerifiedPattern(x) => &x.statement,
        KnowledgeRecord::ApprovedProcedure(x) => &x.outcome.postconditions,
        KnowledgeRecord::Invalidation(x) => &x.reason,
    }
}
fn build_index(
    catalog: &knowledge::Catalog,
    catalog_digest: &str,
    loaded: &knowledge::Loaded,
    revision: u32,
) -> Result<PostingsIndex> {
    let mut index = PostingsIndex {
        schema: INDEX_SCHEMA.into(),
        schema_version: INDEX_VERSION,
        vault_id: catalog.vault_id.clone(),
        revision,
        catalog_generation: catalog.generation,
        catalog_digest: catalog_digest.into(),
        documents: vec![],
        postings: BTreeMap::new(),
        binding_tag: String::new(),
    };
    let mut count = 0;
    for mapping in &catalog.entries {
        let item = knowledge::require_reference(loaded, &mapping.reference)?;
        let d = document(&item.record, &mapping.reference);
        let i = index.documents.len();
        let weights = lexical_weights(&item.record)?;
        count += weights.len();
        if count > MAX_POSTINGS {
            return Err(RetrievalError::IndexLimit);
        }
        for (term, weight) in weights {
            index.postings.entry(term).or_default().push(Posting {
                document: i,
                weight,
            });
        }
        if index.postings.len() > MAX_TERMS {
            return Err(RetrievalError::IndexLimit);
        }
        index.documents.push(d);
    }
    Ok(index)
}
/// Exact corpus features used both to build and to independently authenticate
/// selected postings. Aliases apply only to queries, never to corpus features.
fn lexical_weights(record: &KnowledgeRecord) -> Result<BTreeMap<String, u16>> {
    let mut weights: BTreeMap<String, u16> = BTreeMap::new();
    // Signed Stage 4 runtime receipts (CheckResult / ProcedureRun / UiApproval
    // evidence whose content is the sealed `runtime-receipt.v1` envelope) are
    // audit evidence reached through the patterns that cite them, not text to
    // recall. Giving them no postings keeps ~9 receipts per verification from
    // consuming the append-only store's global posting/term caps. Their catalog
    // document entry (binding) is unchanged. Same rule at build and at
    // selected-record authentication; other evidence of those kinds is indexed.
    let receipt = matches!(
        record,
        KnowledgeRecord::Evidence(e) if matches!(
            e.evidence_kind,
            EvidenceKind::CheckResult | EvidenceKind::ProcedureRun | EvidenceKind::UiApproval
        ) && is_runtime_receipt(&e.content)
    );
    if record.kind() != KnowledgeKind::Invalidation && !receipt {
        for (text, weight) in std::iter::once((body(record), 1u16))
            .chain(
                record
                    .header()
                    .metadata
                    .applicability
                    .topics
                    .iter()
                    .map(|t| (t.as_str(), 4)),
            )
            .chain(std::iter::once((record.header().logical_id.as_str(), 2)))
        {
            for term in lexical_terms(text) {
                // A term longer than any query can be (e.g. a hex-encoded output
                // blob in a Stage 4 receipt) can never match; skip it. Failing
                // here made one ordinary record block every future rebuild of
                // the append-only store.
                if term.chars().count() > MAX_QUERY_CHARS {
                    continue;
                }
                let current = weights.entry(term).or_default();
                *current = (*current + weight).min(64);
            }
        }
    }
    if weights.len() > MAX_DOCUMENT_TERMS {
        // Deterministic per-record cap (same rule at build and at selected-record
        // authentication): keep the highest-weight terms, ties by term order.
        let mut ranked: Vec<(String, u16)> = weights.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(MAX_DOCUMENT_TERMS);
        weights = ranked.into_iter().collect();
    }
    Ok(weights)
}
/// Exact prefix of a sealed Stage 4 receipt (typed field order: schema first).
fn is_runtime_receipt(content: &str) -> bool {
    content
        .strip_prefix("{\"schema\":\"")
        .and_then(|rest| rest.strip_prefix(crate::knowledge_verification::RECEIPT_SCHEMA))
        .is_some_and(|rest| rest.starts_with("\","))
}
/// Unicode scalar lowercase + whitespace/punctuation tokenization; preserves
/// namespace ':' and identifier '_'. NO NFKC, stemming, fuzzy or vector search.
fn lexical_terms(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut term = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            term.push(c);
        } else if !term.is_empty() {
            result.push(std::mem::take(&mut term));
        }
    }
    if !term.is_empty() {
        result.push(term);
    }
    result
}
/// The only operator-declared paraphrase aliases. They are visible in the audit;
/// not learned, user-specific or a claim of embedding semantic capability.
pub const DECLARED_ALIASES: &[(&str, &str)] = &[
    ("locking", "lock"),
    ("locked", "lock"),
    ("unlocking", "unlock"),
    ("revocation", "revoke"),
    ("revoked", "revoke"),
    ("deleting", "delete"),
];
fn normalize_query(query: &RetrievalQuery) -> Result<NormalizationAudit> {
    if query.text.chars().count() > MAX_QUERY_CHARS
        || query.text.chars().any(char::is_control)
        || !valid_text(&query.platform, 128)
        || query.candidate_limit == 0
        || query.candidate_limit > MAX_TARGETED_CANDIDATES
        || query.result_limit == 0
        || query.result_limit > 16
        || query.result_limit > query.candidate_limit
        || query.context_chars == 0
        || query.context_chars > 16384
        || query.snippet_chars == 0
        || query.snippet_chars > 2048
        || query.topic.as_ref().is_some_and(|t| !valid_text(t, 256))
    {
        return Err(RetrievalError::InvalidQuery);
    }
    if query.mode == RecallMode::Current && query.trusted_source.is_none() {
        return Err(RetrievalError::MissingTrustedVersion);
    }
    if let Some(source) = &query.trusted_source {
        if !valid_source(source, true) {
            return Err(RetrievalError::MissingTrustedVersion);
        }
    }
    let raw = lexical_terms(&query.text);
    if raw.is_empty()
        || raw.len() > MAX_QUERY_TERMS
        || query.kind == QueryKind::Symbol && raw.len() != 1
    {
        return Err(RetrievalError::InvalidQuery);
    }
    let mut terms = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    for mut term in raw {
        if query.kind == QueryKind::Terms {
            if let Some((from, to)) = DECLARED_ALIASES.iter().find(|(from, _)| *from == term) {
                aliases.insert(format!("{from}->{to}"));
                term = (*to).into();
            }
        }
        terms.insert(term);
    }
    Ok(NormalizationAudit {
        algorithm: "unicode-scalar-lowercase/identifier-colon-underscore/and/v1; no embeddings"
            .into(),
        terms: terms.into_iter().collect(),
        declared_aliases_used: aliases.into_iter().collect(),
    })
}
fn valid_text(s: &str, bytes: usize) -> bool {
    !s.trim().is_empty() && s.len() <= bytes && !s.chars().any(char::is_control)
}
fn valid_strings(s: &[String], count: usize, bytes: usize) -> bool {
    !s.is_empty()
        && s.len() <= count
        && s.iter().all(|s| valid_text(s, bytes))
        && s.iter().collect::<BTreeSet<_>>().len() == s.len()
}
fn valid_source(s: &TrustedSource, fence: bool) -> bool {
    valid_text(&s.source_id, 512)
        && valid_text(&s.source_version, 256)
        && (s.source_commit.len() == 40 || s.source_commit.len() == 64)
        && s.source_commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && if fence {
            valid_digest(&s.file_digest)
        } else {
            s.file_digest.is_empty()
        }
}
fn metadata_applicable(m: &KnowledgeMetadata, q: &RetrievalQuery) -> bool {
    let Some(source) = &q.trusted_source else {
        return false;
    };
    let Ok(fence) = serde_json::from_str::<ApplicabilityFence>(&m.applicability.constraints) else {
        return false;
    };
    fence.schema == FENCE_SCHEMA
        && fence.file_digest == source.file_digest
        && m.source_id == source.source_id
        && m.source_version == source.source_version
        && m.source_commit == source.source_commit
        && m.applicability.platforms.contains(&q.platform)
}
fn eligible(d: &Document, q: &RetrievalQuery) -> bool {
    let scope =
        d.platforms.contains(&q.platform) && q.topic.as_ref().is_none_or(|t| d.topics.contains(t));
    if q.mode == RecallMode::Historical {
        return scope;
    }
    scope && d.has_fence && q.trusted_source.as_ref() == Some(&d.source)
}
fn candidates(
    index: &PostingsIndex,
    query: &RetrievalQuery,
    audit: &NormalizationAudit,
) -> (Vec<(usize, u32)>, usize) {
    // A source-current fence does not make an older logical revision current.
    // Keep Stage2 audit/active semantics unchanged; current recall selects heads.
    let heads: BTreeMap<_, _> = index
        .documents
        .iter()
        .map(|d| (d.reference.logical_id.as_str(), d.reference.revision))
        .collect();
    let mut scores = BTreeMap::new();
    for (n, term) in audit.terms.iter().enumerate() {
        let list = index.postings.get(term).map_or(&[][..], Vec::as_slice);
        let current: BTreeMap<_, _> = list
            .iter()
            .filter(|p| {
                let d = &index.documents[p.document];
                eligible(d, query)
                    && (query.mode == RecallMode::Historical
                        || heads.get(d.reference.logical_id.as_str())
                            == Some(&d.reference.revision))
            })
            .map(|p| (p.document, u32::from(p.weight)))
            .collect();
        if n == 0 {
            scores = current;
        } else {
            scores.retain(|id, weight| {
                if let Some(w) = current.get(id) {
                    *weight += w;
                    true
                } else {
                    false
                }
            });
        }
    }
    let total = scores.len();
    let mut candidates: Vec<_> = scores
        .into_iter()
        .map(|(id, score)| {
            let boost = match index.documents[id].reference.kind {
                KnowledgeKind::VerifiedPattern => 300,
                KnowledgeKind::ApprovedProcedure => 250,
                KnowledgeKind::Candidate => 200,
                KnowledgeKind::Evidence => 100,
                KnowledgeKind::Invalidation => 0,
            };
            (id, score + boost)
        })
        .collect();
    candidates.sort_by(|(a, x), (b, y)| {
        y.cmp(x).then_with(|| {
            index.documents[*a]
                .reference
                .cmp(&index.documents[*b].reference)
        })
    });
    candidates.truncate(query.candidate_limit);
    (candidates, total)
}
fn render_hits(
    index: &PostingsIndex,
    query: &RetrievalQuery,
    normalization: NormalizationAudit,
    candidates: Vec<(usize, u32)>,
    total: usize,
    selected: TargetedRead,
) -> Result<RecallResult> {
    let mut hits = Vec::new();
    let mut context = 0;
    for (item, (i, score)) in selected.items.into_iter().zip(&candidates) {
        if query.mode == RecallMode::Current
            && (!item.active
                || item.contradictory
                || item
                    .dependency_metadata
                    .iter()
                    .any(|m| !metadata_applicable(m, query)))
        {
            continue;
        }
        let d = &index.documents[*i];
        let mut hit = RecallHit {
            reference: d.reference.clone(), source: d.source.clone(), platforms: d.platforms.clone(), topics: d.topics.clone(),
            provenance: item.stored.mapping.references, snippet: body(&item.stored.record).chars().take(query.snippet_chars).collect(),
            score: *score, why_recalled: format!("exact AND postings: {}; kind-priority then bounded lexical weight; {}; metadata never execution authority",
                normalization.terms.join(", "), if query.mode == RecallMode::Current { "exact trusted source/platform/file fence; logical head; active and noncontradictory" } else { "explicit historical audit; may be stale, revoked or contradictory" }),
            active: item.active, contradictory: item.contradictory, mode: query.mode,
        };
        let remaining = query.context_chars - context;
        loop {
            let size = serde_json::to_string(&hit)
                .map_err(|_| KnowledgeStoreError::Corrupt)?
                .chars()
                .count();
            if size <= remaining {
                context += size;
                hits.push(hit);
                break;
            }
            if hit.snippet.is_empty() {
                break;
            }
            // Unicode-safe and accounts for escaping in serialized context.
            hit.snippet.pop();
        }
        if hits.len() >= query.result_limit {
            break;
        }
    }
    Ok(RecallResult {
        catalog_generation: index.catalog_generation,
        normalization,
        hits,
        stats: RetrievalStats {
            catalog_entries: index.documents.len(),
            posting_candidates: total,
            selected_candidates: candidates.len(),
            payload_decryptions: selected.payload_decryptions,
            control_decryptions: 4,
            context_chars: context,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    const PASSWORD: &[u8] = b"stage3-synthetic-compound-fixture";
    const COMMIT: &str = "aef047b7b0c0cfb57f1f56ed2be33affcf60247e";
    fn source() -> TrustedSource {
        TrustedSource {
            source_id: "PRIVATE-source-path/knowledge.rs".into(),
            source_version: "v3-fixture".into(),
            source_commit: COMMIT.into(),
            file_digest: knowledge::digest(b"fixture-source-file"),
        }
    }
    fn header(id: &str) -> RecordHeader {
        let s = source();
        RecordHeader {
            schema: KNOWLEDGE_SCHEMA.into(),
            logical_id: id.into(),
            revision: 1,
            previous: None,
            timestamp_ms: 1,
            audit: Audit {
                actor: "PRIVATE-actor".into(),
                reason: "PRIVATE-reason".into(),
            },
            metadata: KnowledgeMetadata {
                source_id: s.source_id,
                source_version: s.source_version,
                source_commit: s.source_commit,
                license: "PRIVATE-license".into(),
                privacy: KnowledgePrivacy::Private,
                applicability: Applicability {
                    topics: vec!["PRIVATE-vault-topic".into()],
                    platforms: vec!["linux".into()],
                    constraints: ApplicabilityFence {
                        schema: FENCE_SCHEMA.into(),
                        file_digest: s.file_digest,
                    }
                    .encode()
                    .unwrap(),
                },
            },
            edges: vec![],
        }
    }
    fn evidence(id: &str, content: &str) -> KnowledgeRecord {
        KnowledgeRecord::Evidence(Evidence {
            header: header(id),
            evidence_kind: EvidenceKind::CheckResult,
            content: content.into(),
            content_sha256: knowledge::digest(content.as_bytes()),
        })
    }
    fn candidate(id: &str, statement: &str, r: &RecordRef) -> KnowledgeRecord {
        KnowledgeRecord::Candidate(Candidate {
            header: header(id),
            statement: statement.into(),
            evidence: vec![r.clone()],
        })
    }
    fn query(text: &str) -> RetrievalQuery {
        RetrievalQuery {
            text: text.into(),
            kind: QueryKind::Terms,
            mode: RecallMode::Current,
            platform: "linux".into(),
            trusted_source: Some(source()),
            topic: None,
            candidate_limit: 8,
            result_limit: 4,
            context_chars: 8192,
            snippet_chars: 128,
        }
    }
    fn fixture() -> (
        tempfile::TempDir,
        Arc<Mutex<Option<Vault>>>,
        EvidenceVault,
        KnowledgeRetriever,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UNOONE");
        Vault::create(&root, PASSWORD).unwrap();
        let mut v = Vault::open(&root).unwrap();
        v.unlock(PASSWORD).unwrap();
        let shared = Arc::new(Mutex::new(Some(v)));
        let store = EvidenceVault::new(shared.clone());
        store.initialize().unwrap();
        let retrieval = KnowledgeRetriever::new(shared.clone());
        (temp, shared, store, retrieval)
    }
    fn path(temp: &tempfile::TempDir, id: &str) -> PathBuf {
        temp.path()
            .join("UNOONE/VAULT/records")
            .join(format!("{id}.enc.json"))
    }
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        for entry in std::fs::read_dir(root).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                files.extend(snapshot(&p));
            } else {
                files.insert(p.clone(), std::fs::read(p).unwrap());
            }
        }
        files
    }
    fn rewrite_control(
        shared: &Arc<Mutex<Option<Vault>>>,
        uuid: &str,
        field: &str,
        value: serde_json::Value,
    ) {
        let mut guard = shared.lock().unwrap();
        let v = guard.as_mut().unwrap();
        let (mut metadata, bytes) = knowledge::strict_read(v, uuid).unwrap();
        let mut payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Vault write increments the revision; keep this authenticated fixture
        // internally consistent so foreign schema/mapping guards, not AEAD, fire.
        if uuid == KnowledgeRetriever::index_uuid() {
            payload["revision"] = (metadata.revision + 1).into();
        } else {
            metadata.revision -= 1;
        }
        payload[field] = value;
        // Preserve canonical shape when mutating catalog/index fields: these
        // probes must exercise the publication MAC, not just byte-order rejection.
        let bytes = if uuid == KnowledgeRetriever::index_uuid() {
            serde_json::from_value::<PostingsIndex>(payload.clone())
                .ok()
                .map(|x| serde_json::to_vec(&x).unwrap())
        } else if uuid == EvidenceVault::catalog_uuid() {
            serde_json::from_value::<knowledge::Catalog>(payload.clone())
                .ok()
                .map(|x| serde_json::to_vec(&x).unwrap())
        } else {
            None
        }
        .unwrap_or_else(|| serde_json::to_vec(&payload).unwrap());
        v.write_record(metadata, &bytes).unwrap();
    }
    fn tamper_cipher(file: &Path) {
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        let text = raw["encrypted_content"].as_str().unwrap();
        let mut s = text.to_string();
        s.replace_range(..1, if text.starts_with('0') { "1" } else { "0" });
        assert_ne!(
            decoded_hex(text),
            decoded_hex(&s),
            "decoded bytes must change"
        );
        raw["encrypted_content"] = s.into();
        std::fs::write(file, serde_json::to_vec(&raw).unwrap()).unwrap();
    }
    fn baseline(retrieval: &KnowledgeRetriever, q: &RetrievalQuery) -> RecallResult {
        let audit = normalize_query(q).unwrap();
        retrieval
            .store
            .with_vault(|v| {
                let (catalog, metadata) = knowledge::load_catalog(v)?;
                let loaded = knowledge::load_all(v, &catalog)?;
                // Full-scan baseline: decrypt all payloads and derive the SAME lexical
                // features/eligibility/ranking every request, without persistent writes.
                let index = build_index(&catalog, &metadata.content_hash, &loaded, 2).unwrap();
                let (candidates, total) = candidates(&index, q, &audit);
                let refs: Vec<_> = candidates
                    .iter()
                    .map(|(i, _)| index.documents[*i].reference.clone())
                    .collect();
                let selected = knowledge::selected_from_loaded(&catalog, &loaded, &refs)?;
                Ok(render_hits(&index, q, audit, candidates, total, selected).unwrap())
            })
            .unwrap()
    }
    fn percentiles(mut micros: Vec<u128>) -> (u128, u128) {
        micros.sort();
        (
            micros[(micros.len() - 1) * 50 / 100],
            micros[(micros.len() * 95).div_ceil(100) - 1],
        )
    }

    fn restore(root: &Path, files: &BTreeMap<PathBuf, Vec<u8>>) {
        for p in snapshot(root).keys() {
            if !files.contains_key(p) {
                std::fs::remove_file(p).unwrap();
            }
        }
        for (p, bytes) in files {
            std::fs::write(p, bytes).unwrap();
        }
    }
    fn decoded_hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn knowledge_retrieval_long_terms_and_term_cap_never_block_indexing() {
        // Stage 6 finding: a Stage 4 receipt with >128 output bytes contains a
        // hex token >256 scalars, and a long document has >512 distinct terms.
        // Both used to return IndexLimit from the shared feature extractor and
        // permanently block rebuild of the append-only store.
        let hexblob = "ab".repeat(400);
        let many: Vec<String> = (0..700).map(|i| format!("tok{i:04}")).collect();
        let body = format!("{hexblob} keepword {}", many.join(" "));
        let rec = evidence("long-terms", &body);
        let a = lexical_weights(&rec).expect("never IndexLimit for one record");
        let b = lexical_weights(&rec).unwrap();
        assert_eq!(a, b, "deterministic: build and authentication agree");
        assert_eq!(a.len(), MAX_DOCUMENT_TERMS);
        assert!(a.keys().all(|t| t.chars().count() <= MAX_QUERY_CHARS));
        assert!(!a.contains_key(&hexblob));
        // Higher-weight terms (logical id, weight 2) survive the cap.
        assert!(
            a.contains_key("long"),
            "{:?}",
            a.keys().take(5).collect::<Vec<_>>()
        );
        // A short record is unchanged by the rule.
        let small = lexical_weights(&evidence("small", "alpha beta")).unwrap();
        assert!(small.contains_key("alpha") && small.contains_key("beta"));
        // Signed Stage 4 receipts carry no postings (global-cap headroom);
        // other evidence of the same kinds (no receipt envelope) is indexed.
        let sealed = format!(
            "{{\"schema\":\"{}\",\"logical_id\":\"check-1\",\"body\":\"alpha\"}}",
            crate::knowledge_verification::RECEIPT_SCHEMA
        );
        for kind in [
            EvidenceKind::CheckResult,
            EvidenceKind::ProcedureRun,
            EvidenceKind::UiApproval,
        ] {
            let mut r = evidence("receipt", &sealed);
            if let KnowledgeRecord::Evidence(e) = &mut r {
                e.evidence_kind = kind;
            }
            assert!(lexical_weights(&r).unwrap().is_empty(), "{kind:?}");
            let mut plain = evidence("plain", "alpha beta receiptword");
            if let KnowledgeRecord::Evidence(e) = &mut plain {
                e.evidence_kind = kind;
            }
            assert!(lexical_weights(&plain).unwrap().contains_key("receiptword"));
        }
    }
    #[test]
    fn knowledge_retrieval_bounded_declared_normalization() {
        // B3: every initial hex nibble, including lowercase a, must change bytes.
        // No Vault hex-case compatibility change is required or permitted.
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("synthetic-ciphertext.json");
        for first in "0123456789abcdefABCDEF".chars() {
            let text = format!("{first}0ff");
            std::fs::write(
                &file,
                serde_json::to_vec(&serde_json::json!({"encrypted_content":text})).unwrap(),
            )
            .unwrap();
            tamper_cipher(&file);
            let changed: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            assert_ne!(
                decoded_hex(&text),
                decoded_hex(changed["encrypted_content"].as_str().unwrap()),
                "B3 case-only mutation for initial nibble {first}"
            );
        }
        let mut q = query("LOCKING Vault::LOCK");
        let n = normalize_query(&q).unwrap();
        assert_eq!(n.terms, ["lock", "vault::lock"]);
        assert_eq!(n.declared_aliases_used, ["locking->lock"]);
        q.kind = QueryKind::Symbol;
        q.text = "Vault::LOCK".into();
        assert_eq!(normalize_query(&q).unwrap().terms, ["vault::lock"]);
        q.text = "locked".into();
        assert_eq!(normalize_query(&q).unwrap().terms, ["locked"]);
        q.text = "锁".repeat(256);
        assert!(normalize_query(&q).is_ok());
        q.text.push('锁');
        assert_eq!(normalize_query(&q), Err(RetrievalError::InvalidQuery));
        q.text = "lock\0".into();
        assert!(normalize_query(&q).is_err());
        q.text = "   ".into();
        assert!(normalize_query(&q).is_err());
        q.text = "one two".into();
        assert!(normalize_query(&q).is_err());
        q.kind = QueryKind::Terms;
        q.text = (0..17).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        assert!(normalize_query(&q).is_err());
        q = query("lock");
        q.trusted_source = None;
        assert_eq!(
            normalize_query(&q),
            Err(RetrievalError::MissingTrustedVersion)
        );
        q.mode = RecallMode::Historical;
        assert!(normalize_query(&q).is_ok());
        q.candidate_limit = 65;
        assert!(normalize_query(&q).is_err());
        q = query("lock");
        q.trusted_source.as_mut().unwrap().source_version.clear();
        assert_eq!(
            normalize_query(&q),
            Err(RetrievalError::MissingTrustedVersion)
        );
        let uuids = [
            KnowledgeRetriever::index_uuid(),
            KnowledgeRetriever::anchor_uuid(),
            EvidenceVault::catalog_uuid(),
            EvidenceVault::anchor_uuid(),
        ];
        assert_eq!(uuids.iter().collect::<BTreeSet<_>>().len(), 4);
    }

    #[test]
    fn knowledge_retrieval_real_vault_targeting_fences_revocation_benchmark() {
        let (temp, shared, store, retrieval) = fixture();
        assert_eq!(
            retrieval.search(&query("Vault::lock")),
            Err(RetrievalError::UninitializedIndex)
        );
        // At least 64 unrelated encrypted records; representative 2KiB bodies.
        let mut unrelated = Vec::new();
        for i in 0..64 {
            unrelated.push(
                store
                    .create(evidence(
                        &format!("noise-{i}"),
                        &format!("noise{i} {}", "PRIVATE-unrelated-payload ".repeat(80)),
                    ))
                    .unwrap(),
            );
        }
        let e = store
            .create(evidence(
                "lock-evidence",
                "Vault::lock lock PRIVATE-selected-payload 🔐 लॉक",
            ))
            .unwrap();
        let c = store
            .create(candidate(
                "lock-candidate",
                "Vault::lock lock PRIVATE-advice 🔐 लॉक",
                &e.reference,
            ))
            .unwrap();
        let p = store
            .create(KnowledgeRecord::VerifiedPattern(VerifiedPattern {
                header: header("lock-pattern"),
                candidate: c.reference.clone(),
                checks: vec![e.reference.clone()],
                statement: "Vault::lock lock PRIVATE-pattern 🔐 लॉक".into(),
            }))
            .unwrap();
        let build = retrieval.rebuild().unwrap();
        assert_eq!(build.documents, 67);
        assert_eq!(build.payload_decryptions, 67);
        let q = query("Vault::lock");
        let before = snapshot(temp.path());
        let found = retrieval.search(&q).unwrap();
        assert_eq!(found.hits.len(), 3);
        assert_eq!(found.hits[0].reference, p.reference);
        assert_eq!(found.stats.selected_candidates, 3);
        assert_eq!(found.stats.payload_decryptions, 3);
        assert_eq!(found.stats.control_decryptions, 4);
        assert!(found.stats.payload_decryptions < found.stats.catalog_entries);
        assert_eq!(before, snapshot(temp.path()));
        let alias = retrieval.search(&query("locking")).unwrap();
        assert_eq!(alias.hits.len(), 3);
        assert_eq!(alias.normalization.declared_aliases_used, ["locking->lock"]);
        let mut symbol = q.clone();
        symbol.kind = QueryKind::Symbol;
        assert_eq!(retrieval.search(&symbol).unwrap().hits, found.hits);
        let mut bounded = q.clone();
        bounded.context_chars = 1;
        assert!(retrieval.search(&bounded).unwrap().hits.is_empty());
        bounded.context_chars = 3000;
        bounded.snippet_chars = 1;
        let r = retrieval.search(&bounded).unwrap();
        assert!(r.stats.context_chars <= 3000);
        assert!(r.hits.iter().all(|h| h.snippet.chars().count() <= 1));
        let mut absent = q.clone();
        absent.trusted_source = None;
        assert_eq!(
            retrieval.search(&absent),
            Err(RetrievalError::MissingTrustedVersion)
        );
        for field in ["version", "commit", "digest", "source", "platform", "topic"] {
            let mut stale = q.clone();
            let s = stale.trusted_source.as_mut().unwrap();
            match field {
                "version" => s.source_version = "old".into(),
                "commit" => s.source_commit = "b".repeat(40),
                "digest" => s.file_digest = "c".repeat(64),
                "source" => s.source_id = "other".into(),
                "platform" => stale.platform = "windows".into(),
                _ => stale.topic = Some("unknown".into()),
            }
            let r = retrieval.search(&stale).unwrap();
            assert!(r.hits.is_empty());
            assert_eq!(r.stats.payload_decryptions, 0);
            if field == "version" {
                stale.mode = RecallMode::Historical;
                assert_eq!(retrieval.search(&stale).unwrap().hits.len(), 3);
            }
        }
        // Same tasks baseline and indexed retrieval: 4 tasks x 20 samples each,
        // alternated to reduce ordering bias, 2 untimed warmups; debug build.
        let tasks = [
            q.clone(),
            query("locking"),
            query("PRIVATE-unrelated-payload"),
            query("does_not_exist"),
        ];
        let mut measures = Vec::new();
        for task in tasks {
            for _ in 0..2 {
                baseline(&retrieval, &task);
                retrieval.search(&task).unwrap();
            }
            let mut full = Vec::new();
            let mut targeted = Vec::new();
            let mut counts = None;
            for sample in 0..20 {
                let timed_baseline = || {
                    knowledge::reset_read_counts();
                    let t = Instant::now();
                    let r = baseline(&retrieval, &task);
                    let elapsed = t.elapsed().as_micros();
                    assert_eq!(knowledge::read_counts(), [r.stats.payload_decryptions, 2]);
                    (r, elapsed)
                };
                let timed_target = || {
                    knowledge::reset_read_counts();
                    let t = Instant::now();
                    let r = retrieval.search(&task).unwrap();
                    let elapsed = t.elapsed().as_micros();
                    assert_eq!(
                        knowledge::read_counts(),
                        [r.stats.payload_decryptions, r.stats.control_decryptions]
                    );
                    assert!(r.stats.payload_decryptions < 67);
                    (r, elapsed)
                };
                let ((b, bt), (r, rt)) = if sample % 2 == 0 {
                    (timed_baseline(), timed_target())
                } else {
                    let r = timed_target();
                    let b = timed_baseline();
                    (b, r)
                };
                assert_eq!(b.hits, r.hits);
                assert_eq!(b.normalization, r.normalization);
                assert_eq!(b.stats.payload_decryptions, 67);
                full.push(bt);
                targeted.push(rt);
                counts = Some(r.stats);
            }
            let (bp50, bp95) = percentiles(full.clone());
            let (rp50, rp95) = percentiles(targeted.clone());
            measures.push(serde_json::json!({"query": task.text, "baseline_p50_us":bp50,"baseline_p95_us":bp95,
                "indexed_p50_us":rp50,"indexed_p95_us":rp95,"baseline_samples_us":full,"indexed_samples_us":targeted,
                "baseline_payload_decryptions":67,"indexed_stats":counts}));
        }
        println!(
            "STAGE3_BENCHMARK_JSON={}",
            serde_json::json!({"fixture_records":67,"unrelated_records":64,
            "body_bytes_about":2080,"samples_per_task":20,"warmups_per_task":2,"profile":"cargo test debug; local finite real encrypted Vault; warm filesystem cache",
            "trust_validation":"v2 publication HMAC before candidate and incoming/transitive closure payload reads; actual counters checked at strict_read",
            "tasks":measures})
        );
        // Signed controls isolate unrelated payload damage; full audit/mutations
        // still reject it. Selected and incoming dependency faults remain fatal.
        let noise_path = path(&temp, &unrelated[0].physical_uuid);
        let noise_bytes = std::fs::read(&noise_path).unwrap();
        tamper_cipher(&noise_path);
        assert_eq!(retrieval.search(&q).unwrap().hits, found.hits);
        assert!(store.catalog().is_err());
        assert!(store
            .create(evidence("reject-mutation", "attempt"))
            .is_err());
        std::fs::write(&noise_path, &noise_bytes).unwrap();
        let ep = path(&temp, &e.physical_uuid);
        let eb = std::fs::read(&ep).unwrap();
        tamper_cipher(&ep);
        assert!(retrieval.search(&q).is_err());
        std::fs::write(&ep, &eb).unwrap();
        std::fs::remove_file(&ep).unwrap();
        assert!(retrieval.search(&q).is_err());
        std::fs::write(&ep, &eb).unwrap();
        shared
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .delete_record(&e.physical_uuid, "PAI", "LOCAL")
            .unwrap();
        assert!(retrieval.search(&q).is_err());
        std::fs::write(&ep, &eb).unwrap();
        let bad = RecordRef {
            content_digest: "0".repeat(64),
            ..e.reference.clone()
        };
        assert!(store.read_targeted(&[bad]).is_err());
        assert_eq!(
            store.read_targeted(&vec![e.reference.clone(); 65]),
            Err(KnowledgeStoreError::Limit)
        );
        // Catalog mapping/ref corruption independently authenticated by Vault
        // must fail closed, including omitted incoming facts via the full audit.
        let cp = path(&temp, &EvidenceVault::catalog_uuid());
        let cb = std::fs::read(&cp).unwrap();
        let mut mappings: serde_json::Value =
            serde_json::to_value(store.catalog().unwrap()).unwrap();
        mappings[65]["references"][0]["content_digest"] = "0".repeat(64).into();
        rewrite_control(&shared, &EvidenceVault::catalog_uuid(), "entries", mappings);
        assert!(retrieval.search(&q).is_err());
        std::fs::write(&cp, &cb).unwrap();
        // Contradicting evidence on a dependency suppresses current advice;
        // full-catalog audit includes the incoming fact even if unmatched.
        let mut contra = evidence("contradiction", "unmatched opposing statement");
        if let KnowledgeRecord::Evidence(x) = &mut contra {
            x.header.edges.push(GraphEdge {
                relation: EdgeKind::Contradicting,
                target: e.reference.clone(),
            });
        }
        let contra = store.create(contra).unwrap();
        let stale_snapshot = snapshot(temp.path());
        assert_eq!(retrieval.search(&q), Err(RetrievalError::StaleIndex));
        assert_eq!(stale_snapshot, snapshot(temp.path()));
        retrieval.rebuild().unwrap();
        let current = retrieval.search(&q).unwrap();
        assert!(current.hits.is_empty());
        assert_eq!(current.stats.payload_decryptions, 4);
        let mut historical = q.clone();
        historical.mode = RecallMode::Historical;
        historical.trusted_source = None;
        let hist = retrieval.search(&historical).unwrap();
        assert_eq!(hist.hits.len(), 3);
        assert!(hist.hits.iter().all(|h| h.contradictory));
        let xp = path(&temp, &contra.physical_uuid);
        let xb = std::fs::read(&xp).unwrap();
        tamper_cipher(&xp);
        assert!(retrieval.search(&historical).is_err());
        std::fs::write(&xp, &xb).unwrap();
        store
            .invalidate(KnowledgeRecord::Invalidation(Invalidation {
                header: header("revoke-lock"),
                target: e.reference.clone(),
                reason: "PRIVATE-revocation".into(),
            }))
            .unwrap();
        assert_eq!(retrieval.search(&q), Err(RetrievalError::StaleIndex));
        retrieval.rebuild().unwrap();
        assert!(retrieval.search(&q).unwrap().hits.is_empty());
        let hist = retrieval.search(&historical).unwrap();
        assert!(hist.hits.iter().all(|h| !h.active));
        assert_eq!(hist.stats.payload_decryptions, 5);
        assert!(matches!(
            store.read_active(&p.reference),
            Err(KnowledgeStoreError::Invalidated)
        ));
        assert!(store.read(&p.reference).is_ok());
        // Legacy constraints stay wire-compatible but cannot supply current advice.
        let mut legacy = evidence("legacy", "legacyword");
        if let KnowledgeRecord::Evidence(x) = &mut legacy {
            x.header.metadata.applicability.constraints = "free-text old constraints".into();
        }
        let legacy = store.create(legacy).unwrap();
        let dep = store
            .create(candidate(
                "new-scope-old-dependency",
                "legacy_dependency",
                &legacy.reference,
            ))
            .unwrap();
        retrieval.rebuild().unwrap();
        assert!(retrieval
            .search(&query("legacyword"))
            .unwrap()
            .hits
            .is_empty());
        assert!(retrieval
            .search(&query("legacy_dependency"))
            .unwrap()
            .hits
            .is_empty());
        let mut h = query("legacy_dependency");
        h.mode = RecallMode::Historical;
        assert_eq!(
            retrieval.search(&h).unwrap().hits[0].reference,
            dep.reference
        );
        // Private identifiers, terms, paths and snippets do not enter plaintext
        // metadata/journal/filenames. Existing generic hashes/timestamps remain.
        for (p, bytes) in snapshot(temp.path()) {
            for marker in [
                "PRIVATE-selected-payload",
                "PRIVATE-source-path",
                "PRIVATE-pattern",
                "PRIVATE-actor",
                "PRIVATE-license",
                "Vault::lock",
                "lock-evidence",
            ] {
                assert!(!p.to_string_lossy().contains(marker));
                assert!(!bytes.windows(marker.len()).any(|w| w == marker.as_bytes()));
            }
        }
        let index_path = path(&temp, &KnowledgeRetriever::index_uuid());
        let index_bytes = std::fs::read(&index_path).unwrap();
        tamper_cipher(&index_path);
        assert!(retrieval.search(&q).is_err());
        assert!(retrieval.rebuild().is_err());
        std::fs::write(&index_path, &index_bytes).unwrap();
        for version in [0, 99] {
            rewrite_control(
                &shared,
                &KnowledgeRetriever::index_uuid(),
                "schema_version",
                version.into(),
            );
            let frozen = snapshot(temp.path());
            assert_eq!(
                retrieval.search(&q),
                Err(RetrievalError::UnsupportedIndexSchema)
            );
            assert_eq!(
                retrieval.rebuild(),
                Err(RetrievalError::UnsupportedIndexSchema)
            );
            assert_eq!(frozen, snapshot(temp.path()));
            std::fs::write(&index_path, &index_bytes).unwrap();
        }
        rewrite_control(
            &shared,
            &KnowledgeRetriever::index_uuid(),
            "foreign_field",
            true.into(),
        );
        assert!(retrieval.search(&q).is_err());
        assert!(retrieval.rebuild().is_err());
        std::fs::write(&index_path, &index_bytes).unwrap();
        rewrite_control(
            &shared,
            &KnowledgeRetriever::index_uuid(),
            "catalog_digest",
            "0".repeat(64).into(),
        );
        assert_eq!(
            retrieval.search(&q),
            Err(RetrievalError::Store(KnowledgeStoreError::Corrupt))
        );
        let wrong_binding = snapshot(temp.path());
        assert!(retrieval.rebuild().is_err()); // not a valid historical catalog prefix
        assert_eq!(wrong_binding, snapshot(temp.path()));
        std::fs::write(&index_path, &index_bytes).unwrap();
        shared
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .delete_record(&KnowledgeRetriever::index_uuid(), "PAI", "LOCAL")
            .unwrap();
        assert!(retrieval.search(&q).is_err());
        assert!(retrieval.rebuild().is_err());
        std::fs::write(&index_path, &index_bytes).unwrap();
        std::fs::remove_file(&index_path).unwrap();
        assert!(retrieval.search(&q).is_err());
        assert!(retrieval.rebuild().is_err());
        std::fs::write(&index_path, &index_bytes).unwrap();
        let anchor_path = path(&temp, &KnowledgeRetriever::anchor_uuid());
        let anchor_bytes = std::fs::read(&anchor_path).unwrap();
        std::fs::remove_file(&anchor_path).unwrap();
        assert!(retrieval.search(&q).is_err());
        assert!(retrieval.rebuild().is_err());
        std::fs::write(&anchor_path, &anchor_bytes).unwrap();
        let cat_anchor = path(&temp, &EvidenceVault::anchor_uuid());
        let cat_anchor_bytes = std::fs::read(&cat_anchor).unwrap();
        std::fs::remove_file(&cat_anchor).unwrap();
        assert!(retrieval.search(&q).is_err());
        std::fs::write(&cat_anchor, &cat_anchor_bytes).unwrap();
        shared.lock().unwrap().as_mut().unwrap().lock().unwrap();
        assert_eq!(
            retrieval.search(&q),
            Err(RetrievalError::Store(KnowledgeStoreError::Locked))
        );
        assert_eq!(
            retrieval.rebuild(),
            Err(RetrievalError::Store(KnowledgeStoreError::Locked))
        );
        assert_eq!(
            store.read_targeted(&[e.reference]),
            Err(KnowledgeStoreError::Locked)
        );
        *shared.lock().unwrap() = None;
        assert_eq!(
            retrieval.search(&q),
            Err(RetrievalError::Store(KnowledgeStoreError::Locked))
        );
    }

    #[test]
    fn knowledge_retrieval_repair_regressions() {
        // One additional encrypted fixture for B1/B2/B4, with snapshots restored
        // between faults. Keep all red failures visible in one test-first run.
        let (temp, shared, store, retrieval) = fixture();
        let mut failures = Vec::new();
        let empty = snapshot(temp.path());
        let mut malformed_scope = evidence("valid-stage2", "ordinarybody");
        if let KnowledgeRecord::Evidence(x) = &mut malformed_scope {
            x.header.metadata.applicability.topics = vec!["valid-topic\ncontinued".into()];
        }
        assert!(malformed_scope.validate().is_ok());
        store.create(malformed_scope.clone()).unwrap();
        assert!(store.catalog().is_ok());
        let before = snapshot(temp.path());
        assert!(retrieval.rebuild().is_err());
        if snapshot(temp.path()) != before {
            failures.push("B4 first-build rejection wrote index/anchor");
        }
        restore(temp.path(), &empty);
        let e = store.create(evidence("support", "supporttext")).unwrap();
        let c = store
            .create(candidate(
                "selected",
                "selectedword selectedword",
                &e.reference,
            ))
            .unwrap();
        let noise = store.create(evidence("noise", "unrelatedtext")).unwrap();
        retrieval.rebuild().unwrap();
        let valid = snapshot(temp.path());
        store.create(malformed_scope).unwrap();
        let before = snapshot(temp.path());
        for _ in 0..2 {
            assert!(retrieval.rebuild().is_err());
            if snapshot(temp.path()) != before {
                failures.push("B4 replacement rejection changed existing index");
            }
        }
        restore(temp.path(), &valid);

        // Syntactically bounded, AEAD-authenticated derivatives are not proofs
        // of content. Check fabricated terms, wrong valid weights, omitted actual
        // features and non-query poison, including AND and historical/symbol.
        let uuid = KnowledgeRetriever::index_uuid();
        let original: serde_json::Value = {
            let guard = shared.lock().unwrap();
            serde_json::from_slice(&guard.as_ref().unwrap().read_record(&uuid).unwrap().1).unwrap()
        };
        for fault in ["absent", "weight", "omitted", "nonquery"] {
            let mut postings = original["postings"].clone();
            let text = match fault {
                "absent" => {
                    postings["nonexistentterm"] = serde_json::json!([{"document":1,"weight":64}]);
                    "selectedword nonexistentterm"
                }
                "weight" => {
                    postings["selectedword"][0]["weight"] = 64.into();
                    "selectedword"
                }
                "omitted" => {
                    postings.as_object_mut().unwrap().remove("selected");
                    "selectedword"
                }
                _ => {
                    postings["forged_unqueried"] = serde_json::json!([{"document":1,"weight":1}]);
                    "selectedword"
                }
            };
            rewrite_control(&shared, &uuid, "postings", postings);
            let frozen = snapshot(temp.path());
            for mode in [RecallMode::Current, RecallMode::Historical] {
                let mut q = query(text);
                q.mode = mode;
                if retrieval.search(&q).is_ok() {
                    failures.push(match fault {
                        "absent" => "B1 absent AND term admitted",
                        "weight" => "B1 supplied lexical weight trusted",
                        "omitted" => "B1 omitted selected feature not detected",
                        _ => "B1 non-query selected feature forged",
                    });
                }
            }
            let mut symbol = query(if fault == "absent" {
                "nonexistentterm"
            } else {
                text
            });
            symbol.kind = QueryKind::Symbol;
            if retrieval.search(&symbol).is_ok() {
                failures.push("B1 symbol selected derivative not authenticated");
            }
            assert!(retrieval.rebuild().is_err());
            assert_eq!(snapshot(temp.path()), frozen);
            restore(temp.path(), &valid);
        }

        // B2: hide an incoming Evidence contradicting edge, leaving either no
        // references or a plausible nonempty backwards reference to noise.
        for relation in [EdgeKind::Contradicting, EdgeKind::Superseding] {
            let mut incoming = evidence("incoming-fact", "unmatchedbody");
            if let KnowledgeRecord::Evidence(x) = &mut incoming {
                x.header.edges.push(GraphEdge {
                    relation,
                    target: c.reference.clone(),
                });
                x.header.edges.push(GraphEdge {
                    relation: EdgeKind::Supporting,
                    target: noise.reference.clone(),
                });
            }
            store.create(incoming).unwrap();
            retrieval.rebuild().unwrap();
            assert!(retrieval
                .search(&query("selectedword"))
                .unwrap()
                .hits
                .is_empty());
            let complete = snapshot(temp.path());
            for keep_noise in [false, true] {
                let mut entries = serde_json::to_value(store.catalog().unwrap()).unwrap();
                entries[3]["references"] = if keep_noise {
                    serde_json::json!([noise.reference.clone()])
                } else {
                    serde_json::json!([])
                };
                rewrite_control(&shared, &EvidenceVault::catalog_uuid(), "entries", entries);
                assert!(store.read_active(&c.reference).is_err());
                if store
                    .read_targeted(std::slice::from_ref(&c.reference))
                    .is_ok()
                {
                    failures.push("B2 hidden incoming Evidence mapping admitted");
                }
                let digest = {
                    let guard = shared.lock().unwrap();
                    guard
                        .as_ref()
                        .unwrap()
                        .read_record(&EvidenceVault::catalog_uuid())
                        .unwrap()
                        .0
                        .content_hash
                };
                rewrite_control(&shared, &uuid, "catalog_digest", digest.into());
                let frozen = snapshot(temp.path());
                if retrieval.search(&query("selectedword")).is_ok() {
                    failures.push("B2 hidden incoming edge reactivated current advice");
                }
                let mut historical = query("selectedword");
                historical.mode = RecallMode::Historical;
                if retrieval.search(&historical).is_ok() {
                    failures.push("B2 malformed incoming mapping admitted as audit history");
                }
                assert!(retrieval.rebuild().is_err());
                assert_eq!(snapshot(temp.path()), frozen);
                restore(temp.path(), &complete);
            }
            restore(temp.path(), &valid);
        }
        let mut revocation_header = header("incoming-revocation");
        revocation_header.edges.push(GraphEdge {
            relation: EdgeKind::Supporting,
            target: noise.reference.clone(),
        });
        store
            .invalidate(KnowledgeRecord::Invalidation(Invalidation {
                header: revocation_header,
                target: e.reference.clone(),
                reason: "unmatched revocation".into(),
            }))
            .unwrap();
        retrieval.rebuild().unwrap();
        assert!(retrieval
            .search(&query("selectedword"))
            .unwrap()
            .hits
            .is_empty());
        let revoked = snapshot(temp.path());
        for keep_noise in [false, true] {
            let mut entries = serde_json::to_value(store.catalog().unwrap()).unwrap();
            entries[3]["references"] = if keep_noise {
                serde_json::json!([noise.reference.clone()])
            } else {
                serde_json::json!([])
            };
            rewrite_control(&shared, &EvidenceVault::catalog_uuid(), "entries", entries);
            assert!(store.read_active(&c.reference).is_err());
            if store
                .read_targeted(std::slice::from_ref(&c.reference))
                .is_ok()
            {
                failures.push("B2 omitted incoming revocation mapping admitted");
            }
            let digest = {
                let guard = shared.lock().unwrap();
                guard
                    .as_ref()
                    .unwrap()
                    .read_record(&EvidenceVault::catalog_uuid())
                    .unwrap()
                    .0
                    .content_hash
            };
            rewrite_control(&shared, &uuid, "catalog_digest", digest.into());
            let frozen = snapshot(temp.path());
            if retrieval.search(&query("selectedword")).is_ok() {
                failures.push("B2 omitted revocation reactivated current advice");
            }
            assert!(retrieval.rebuild().is_err());
            assert_eq!(snapshot(temp.path()), frozen);
            restore(temp.path(), &revoked);
        }
        println!("STAGE3_REPAIR_REGRESSION_FAILURES={failures:?}");
        assert!(failures.is_empty(), "repair regressions: {failures:?}");
    }

    #[test]
    fn knowledge_retrieval_sealed_controls_before_any_candidate_read() {
        let (temp, shared, store, retrieval) = fixture();
        let support = store
            .create(evidence("seal-support", "supportbody"))
            .unwrap();
        let selected = store
            .create(candidate("seal-selected", "sealword", &support.reference))
            .unwrap();
        store.create(evidence("seal-noise", "noisebody")).unwrap();
        retrieval.rebuild().unwrap();
        let pristine = snapshot(temp.path());
        let mut failures = Vec::new();
        let found = retrieval.search(&query("sealword")).unwrap();
        if found.stats.payload_decryptions != 2 {
            failures.push("candidate closure still scans unrelated payload");
        }
        if retrieval
            .search(&query("nomatch"))
            .unwrap()
            .stats
            .payload_decryptions
            != 0
        {
            failures.push("zero-candidate query scans payloads");
        }
        let index_id = KnowledgeRetriever::index_uuid();
        let catalog_id = EvidenceVault::catalog_uuid();
        let tags = {
            let g = shared.lock().unwrap();
            [catalog_id.clone(), index_id.clone()].map(|id| {
                let json: serde_json::Value =
                    serde_json::from_slice(&g.as_ref().unwrap().read_record(&id).unwrap().1)
                        .unwrap();
                json["binding_tag"].as_str().unwrap().to_owned()
            })
        };
        for uuid in [
            catalog_id.clone(),
            index_id.clone(),
            EvidenceVault::anchor_uuid(),
            KnowledgeRetriever::anchor_uuid(),
        ] {
            let original = {
                let g = shared.lock().unwrap();
                g.as_ref().unwrap().read_record(&uuid).unwrap().1
            };
            let json: serde_json::Value = serde_json::from_slice(&original).unwrap();
            if json["schema_version"] != 2
                || json["binding_tag"].as_str().is_none_or(|t| t.len() != 64)
            {
                failures.push("v2 signed control not published");
            }
            // General Vault::write_record has record encryption authority, NOT
            // publication authority. Preserve metadata revision to isolate binding.
            for fault in [
                "reordered",
                "whitespace",
                "removed",
                "tag",
                "domain",
                "legacy",
            ] {
                let tag = json["binding_tag"].as_str().unwrap();
                let tag_field = format!(",\"binding_tag\":\"{tag}\"");
                let canonical = std::str::from_utf8(&original).unwrap();
                let bytes = match fault {
                    "reordered" => serde_json::to_vec(&json).unwrap(),
                    "whitespace" => {
                        let mut b = original.clone();
                        b.push(b' ');
                        b
                    }
                    "removed" => canonical.replace(&tag_field, "").into_bytes(),
                    "tag" => canonical.replace(tag, &"0".repeat(64)).into_bytes(),
                    "domain" => canonical
                        .replace(tag, &tags[usize::from(uuid == catalog_id)])
                        .into_bytes(),
                    "legacy" => canonical
                        .replace("\"schema_version\":2", "\"schema_version\":1")
                        .replace(&tag_field, "")
                        .into_bytes(),
                    _ => unreachable!(),
                };
                {
                    let mut g = shared.lock().unwrap();
                    let v = g.as_mut().unwrap();
                    let (mut m, _) = v.read_record(&uuid).unwrap();
                    m.revision -= 1;
                    v.write_record(m, &bytes).unwrap();
                }
                let frozen = snapshot(temp.path());
                // Including zero matches: binding must be checked before selecting
                // candidates, not only after returned hits are chosen.
                for text in ["sealword", "nomatch"] {
                    knowledge::reset_read_counts();
                    if retrieval.search(&query(text)).is_ok() {
                        failures.push("modified control accepted before selection");
                    }
                    assert_eq!(
                        knowledge::read_counts()[0],
                        0,
                        "binding failure before payloads"
                    );
                }
                if retrieval.rebuild().is_ok() {
                    failures.push("modified control silently rebuilt");
                }
                if uuid == catalog_id || uuid == EvidenceVault::anchor_uuid() {
                    if store.initialize().is_ok() {
                        failures.push("legacy/modified catalog adopted by initialization");
                    }
                    if store
                        .read_targeted(std::slice::from_ref(&selected.reference))
                        .is_ok()
                    {
                        failures.push("modified catalog accepted by targeted read");
                    }
                }
                if snapshot(temp.path()) != frozen {
                    failures.push("control rejection wrote files");
                }
                restore(temp.path(), &pristine);
            }
        }
        // Unselected posting omission and valid unselected injection must fail
        // before zero-hit queries; selected feature checking alone cannot do this.
        let original: serde_json::Value = {
            let g = shared.lock().unwrap();
            serde_json::from_slice(&g.as_ref().unwrap().read_record(&index_id).unwrap().1).unwrap()
        };
        for remove in [true, false] {
            let mut postings = original["postings"].clone();
            if remove {
                postings.as_object_mut().unwrap().remove("sealword");
            } else {
                postings["unselected_forgery"] = serde_json::json!([{"document":2,"weight":1}]);
            }
            rewrite_control(&shared, &index_id, "postings", postings);
            let frozen = snapshot(temp.path());
            knowledge::reset_read_counts();
            if retrieval.search(&query("nomatch")).is_ok() {
                failures.push("unselected posting tamper accepted");
            }
            assert_eq!(knowledge::read_counts()[0], 0);
            assert!(retrieval.rebuild().is_err());
            assert_eq!(snapshot(temp.path()), frozen);
            restore(temp.path(), &pristine);
        }
        {
            let guard = shared.lock().unwrap();
            let v = guard.as_ref().unwrap();
            let domains = [
                ControlDomain::Catalog,
                ControlDomain::CatalogAnchor,
                ControlDomain::Index,
                ControlDomain::IndexAnchor,
            ];
            for (i, domain) in domains.iter().copied().enumerate() {
                let tag =
                    knowledge::sign_binding(v, domain, b"same canonical control bytes").unwrap();
                for (j, other) in domains.iter().copied().enumerate() {
                    assert_eq!(
                        knowledge::verify_binding(v, other, b"same canonical control bytes", &tag)
                            .is_ok(),
                        i == j
                    );
                }
            }
        }
        // A valid controller mutation makes the old authentic index stale. No
        // payload reads or writes may be used to heal it during normal search.
        store.create(evidence("seal-new", "newbody")).unwrap();
        let frozen = snapshot(temp.path());
        knowledge::reset_read_counts();
        assert_eq!(
            retrieval.search(&query("sealword")),
            Err(RetrievalError::StaleIndex)
        );
        assert_eq!(knowledge::read_counts(), [0, 4]);
        assert!(snapshot(temp.path()) == frozen);
        retrieval.rebuild().unwrap();
        // No retained binding key/cache bypasses a lock; reopening derives keys
        // anew from the owning unlocked Vault, and preserves signed snapshots.
        shared.lock().unwrap().as_mut().unwrap().lock().unwrap();
        assert!(shared
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .master_key()
            .is_none());
        assert_eq!(
            retrieval.search(&query("sealword")),
            Err(RetrievalError::Store(KnowledgeStoreError::Locked))
        );
        let mut reopened = Vault::open(&temp.path().join("UNOONE")).unwrap();
        reopened.unlock(PASSWORD).unwrap();
        *shared.lock().unwrap() = Some(reopened);
        assert_eq!(
            retrieval
                .search(&query("sealword"))
                .unwrap()
                .stats
                .payload_decryptions,
            2
        );
        println!("STAGE3_SEALED_FAILURES={failures:?}");
        assert!(failures.is_empty(), "sealed regressions: {failures:?}");
    }

    #[test]
    fn knowledge_retrieval_real_vault_occupied_anchor_and_superseding() {
        let (temp, shared, store, retrieval) = fixture();
        let uuid = KnowledgeRetriever::index_uuid();
        let mut foreign = knowledge::private_metadata();
        foreign.record_id = uuid.clone();
        shared
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .write_record(foreign, b"foreign private occupied record")
            .unwrap();
        let occupied = snapshot(temp.path());
        assert!(retrieval.rebuild().is_err());
        assert!(retrieval.search(&query("lock")).is_err());
        assert_eq!(occupied, snapshot(temp.path()));
        std::fs::remove_file(path(&temp, &uuid)).unwrap();
        let e = store
            .create(evidence("supersede-evidence", "support"))
            .unwrap();
        let c = store
            .create(candidate("old-candidate", "supersedeword", &e.reference))
            .unwrap();
        let mut next = candidate("successor", "successorword", &e.reference);
        if let KnowledgeRecord::Candidate(x) = &mut next {
            x.header.edges.push(GraphEdge {
                relation: EdgeKind::Superseding,
                target: c.reference.clone(),
            });
        }
        let n = store.create(next).unwrap();
        retrieval.rebuild().unwrap();
        assert!(retrieval
            .search(&query("supersedeword"))
            .unwrap()
            .hits
            .is_empty());
        assert_eq!(
            retrieval.search(&query("successorword")).unwrap().hits[0].reference,
            n.reference
        );
        let mut historical = query("supersedeword");
        historical.mode = RecallMode::Historical;
        assert!(!retrieval.search(&historical).unwrap().hits[0].active);
        // Authentication of audit graph: even unmatched superseding payload
        // deletion/corruption cannot restore the superseded candidate as advice.
        let np = path(&temp, &n.physical_uuid);
        let nb = std::fs::read(&np).unwrap();
        tamper_cipher(&np);
        assert!(retrieval.search(&query("supersedeword")).is_err());
        std::fs::write(&np, &nb).unwrap();
        // Old logical revisions are historical-only even without a Superseding
        // edge. This is a retrieval fence, not a change to Stage2 audit activity.
        let old = store
            .create(candidate("revision-test", "revisionword", &e.reference))
            .unwrap();
        let mut updated = candidate("revision-test", "revisionnew", &e.reference);
        if let KnowledgeRecord::Candidate(x) = &mut updated {
            x.header.revision = 2;
            x.header.previous = Some(old.reference.clone());
        }
        let latest = store.append_revision(updated, &old.reference).unwrap();
        retrieval.rebuild().unwrap();
        assert!(retrieval
            .search(&query("revisionword"))
            .unwrap()
            .hits
            .is_empty());
        assert_eq!(
            retrieval.search(&query("revisionnew")).unwrap().hits[0].reference,
            latest.reference
        );
        let mut audit_old = query("revisionword");
        audit_old.mode = RecallMode::Historical;
        assert_eq!(
            retrieval.search(&audit_old).unwrap().hits[0].reference,
            old.reference
        );
        assert!(store.read_active(&old.reference).is_ok());
        let ap = path(&temp, &KnowledgeRetriever::anchor_uuid());
        let ab = std::fs::read(&ap).unwrap();
        rewrite_control(
            &shared,
            &KnowledgeRetriever::anchor_uuid(),
            "vault_id",
            "foreign-vault".into(),
        );
        let frozen = snapshot(temp.path());
        assert!(retrieval.search(&historical).is_err());
        assert!(retrieval.rebuild().is_err());
        assert_eq!(frozen, snapshot(temp.path()));
        std::fs::write(&ap, &ab).unwrap();
        shared
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .delete_record(&KnowledgeRetriever::anchor_uuid(), "PAI", "LOCAL")
            .unwrap();
        assert!(retrieval.search(&historical).is_err());
        assert!(retrieval.rebuild().is_err());
    }
}
