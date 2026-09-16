use inbharat_harness_core::{
    CancellationToken, ErrorCode, Failure, FailureClass, HarnessResult, MemoryCapabilities,
    MemoryProvider, MemoryQuery, MemoryRecord, MemoryScope,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use unoone_vault_core::{EncryptedRecord, PrivacyLevel, Record, RecordType, Vault, VaultError};

const ENVELOPE_SCHEMA: u32 = 1;
const INDEX_SCHEMA: u32 = 1;
const INDEX_NAMESPACE: &str = "__pai_harness_internal__";
const INDEX_LOGICAL_ID: &str = "memory-index-v1";
const DEFAULT_MAX_REPAIR_RECORDS: usize = 8_192;
const HARD_MAX_REPAIR_RECORDS: usize = 100_000;
const MAX_INDEX_ENTRIES: usize = 100_000;
const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct PaiVaultMemoryProviderConfig {
    pub origin_platform: String,
    pub origin_device_id: String,
    /// Maximum encrypted records inspected when repairing a missing per-scope
    /// memory index. Normal search does not scan the records directory.
    pub max_scan_records: usize,
}

impl Default for PaiVaultMemoryProviderConfig {
    fn default() -> Self {
        Self {
            origin_platform: "POCKET_AI".to_owned(),
            origin_device_id: "unknown-device".to_owned(),
            max_scan_records: DEFAULT_MAX_REPAIR_RECORDS,
        }
    }
}

/// Harness memory backed only by the canonical encrypted Pocket-AI vault.
///
/// There is intentionally no plaintext side database. Stable Harness IDs map
/// to deterministic UUIDv4-shaped vault record IDs, while all user content,
/// attributes and the search index are encrypted by `unoone-vault-core`.
pub struct PaiVaultMemoryProvider {
    vault: Arc<Mutex<Option<Vault>>>,
    config: PaiVaultMemoryProviderConfig,
    /// Gap 5 (2026-09-16): optional model-backed rerank of lexical search
    /// hits (see `with_lexical_rerank`). None keeps the pure lexical order.
    lexical_rerank: Option<LexicalRerank>,
}

/// Gap 5 (2026-09-16): the local model endpoint used to rerank lexical
/// memory hits — the same verified llama-server the chat plane runs on.
pub(crate) struct LexicalRerank {
    model_id: String,
    port: u16,
}

impl std::fmt::Debug for PaiVaultMemoryProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaiVaultMemoryProvider")
            .field("origin_platform", &self.config.origin_platform)
            .field("origin_device_id", &self.config.origin_device_id)
            .field("max_scan_records", &self.config.max_scan_records)
            .field("lexical_rerank", &self.lexical_rerank.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultMemoryEnvelope {
    schema: u32,
    harness_id: String,
    scope: String,
    namespace: String,
    content: String,
    attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultMemoryIndexEntry {
    data_record_id: String,
    harness_id: String,
    namespace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultMemoryIndex {
    schema: u32,
    scope: String,
    entries: BTreeMap<String, VaultMemoryIndexEntry>,
}

impl VaultMemoryIndex {
    fn empty(scope: MemoryScope) -> Self {
        Self {
            schema: INDEX_SCHEMA,
            scope: scope.as_str().to_owned(),
            entries: BTreeMap::new(),
        }
    }
}

impl PaiVaultMemoryProvider {
    pub fn new(
        vault: Arc<Mutex<Option<Vault>>>,
        mut config: PaiVaultMemoryProviderConfig,
    ) -> HarnessResult<Self> {
        if config.origin_platform.trim().is_empty()
            || config.origin_platform.len() > 64
            || config.origin_platform.contains('\0')
            || config.origin_device_id.trim().is_empty()
            || config.origin_device_id.len() > 128
            || config.origin_device_id.contains('\0')
        {
            return Err(Failure::invalid(
                "pai.memory.new",
                "origin platform/device identifiers are invalid",
            ));
        }
        config.max_scan_records = config.max_scan_records.clamp(1, HARD_MAX_REPAIR_RECORDS);
        Ok(Self {
            vault,
            config,
            lexical_rerank: None,
        })
    }

    /// Gap 5 (2026-09-16): install the model-backed rerank. When set,
    /// text-search results with three or more lexical hits are reordered by
    /// one small think-off completion on the verified local model
    /// ("which of these notes is most relevant to the query") before the
    /// harness injects them into the agent context. Fail-open by design: any
    /// rerank error keeps the lexical order — retrieval quality must never
    /// regress below today's behavior.
    #[must_use]
    pub fn with_lexical_rerank(mut self, model_id: impl Into<String>, port: u16) -> Self {
        let model_id = model_id.into();
        if port != 0 && !model_id.trim().is_empty() {
            self.lexical_rerank = Some(LexicalRerank { model_id, port });
        }
        self
    }

    /// One bounded think-off rerank call against the local model. Reorders
    /// `hits` in place (most relevant first) using the model's index ranking;
    /// every failure path returns with the order unchanged.
    fn rerank_lexical_hits(&self, query_text: &str, hits: &mut [MemoryRecord]) {
        let Some(rerank) = &self.lexical_rerank else {
            return;
        };
        // Below three hits there is nothing worth a model call to reorder.
        if hits.len() < 3 {
            return;
        }
        const CANDIDATES: usize = 8;
        const EXCERPT_CHARS: usize = 200;

        let mut prompt = format!(
            "Query: {}\n\nResults:\n",
            query_text.chars().take(400).collect::<String>()
        );
        for (index, record) in hits.iter().take(CANDIDATES).enumerate() {
            let excerpt: String = record
                .content
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .take(EXCERPT_CHARS)
                .collect();
            prompt.push_str(&format!("{}. {}\n", index + 1, excerpt));
        }
        prompt.push_str("\nRank the results by relevance to the query.");

        let body = serde_json::json!({
            "model": rerank.model_id,
            "messages": [
                {"role": "system", "content": "You rank search results by relevance to a query. Reply with ONLY the result numbers, most relevant first, separated by commas (for example: 3,1,2). Never output anything else."},
                {"role": "user", "content": prompt}
            ],
            "max_tokens": 96,
            "temperature": 0.1,
            "stream": false,
            "cache_prompt": true,
            "chat_template_kwargs": { "enable_thinking": false },
            "reasoning_budget": 0,
        });
        let request_bytes = match serde_json::to_vec(&body) {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let cancel = CancellationToken::new();
        // Bounded hard: the memory trait has no cancellation channel, so the
        // call must never outlive this short deadline. On any failure the
        // lexical order stands.
        let response = match crate::llama_local::post_json_localhost(
            rerank.port,
            &request_bytes,
            std::time::Duration::from_secs(30),
            &cancel,
        ) {
            Ok(response) => response,
            Err(_) => return,
        };
        if !(200..300).contains(&(response.status as usize)) {
            return;
        }
        let payload: serde_json::Value = match serde_json::from_slice(&response.body) {
            Ok(value) => value,
            Err(_) => return,
        };
        let ranking_text = payload
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        // Parse "3,1,2" (tolerant of prose around the numbers) into a stable
        // 1-based index order, then apply it: ranked hits first (in the
        // model's order), then everything the model did not list, lexical.
        let mut order: Vec<usize> = Vec::with_capacity(CANDIDATES);
        for token in ranking_text.split(|c: char| !c.is_ascii_digit()) {
            if token.is_empty() {
                continue;
            }
            let Ok(one_based) = token.parse::<usize>() else {
                continue;
            };
            let zero_based = one_based.saturating_sub(1);
            if zero_based < hits.len() && !order.contains(&zero_based) {
                order.push(zero_based);
            }
        }
        if order.is_empty() {
            return;
        }
        let mut remaining: Vec<usize> = (0..hits.len())
            .filter(|index| !order.contains(index))
            .collect();
        order.append(&mut remaining);
        let reordered: Vec<MemoryRecord> =
            order.into_iter().map(|index| hits[index].clone()).collect();
        hits.clone_from_slice(&reordered);
    }

    fn with_vault<R>(
        &self,
        operation: &'static str,
        f: impl FnOnce(&Vault) -> HarnessResult<R>,
    ) -> HarnessResult<R> {
        let guard = self.vault.lock().map_err(|_| {
            Failure::new(
                ErrorCode::ProviderFailed,
                FailureClass::Persistence,
                operation,
                "Pocket AI vault mutex was poisoned",
            )
        })?;
        let vault = guard.as_ref().ok_or_else(|| {
            Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Persistence,
                operation,
                "Pocket AI vault is locked",
            )
        })?;
        if !vault.is_unlocked() {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Persistence,
                operation,
                "Pocket AI vault is locked",
            ));
        }
        f(vault)
    }

    fn with_vault_mut<R>(
        &self,
        operation: &'static str,
        f: impl FnOnce(&mut Vault) -> HarnessResult<R>,
    ) -> HarnessResult<R> {
        let mut guard = self.vault.lock().map_err(|_| {
            Failure::new(
                ErrorCode::ProviderFailed,
                FailureClass::Persistence,
                operation,
                "Pocket AI vault mutex was poisoned",
            )
        })?;
        let vault = guard.as_mut().ok_or_else(|| {
            Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Persistence,
                operation,
                "Pocket AI vault is locked",
            )
        })?;
        if !vault.is_unlocked() {
            return Err(Failure::new(
                ErrorCode::PermissionDenied,
                FailureClass::Persistence,
                operation,
                "Pocket AI vault is locked",
            ));
        }
        f(vault)
    }

    fn record_id(scope: MemoryScope, namespace: &str, id: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"pai-harness-memory-v1\0");
        hasher.update(scope.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(namespace.as_bytes());
        hasher.update(b"\0");
        hasher.update(id.as_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // Vault record IDs are required to be canonical lowercase UUIDv4s. The
        // deterministic digest is reshaped to the UUID v4/variant bit pattern;
        // this is a storage key, not a claim of random UUID generation.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        )
    }

    fn logical_key(scope: MemoryScope, namespace: &str, id: &str) -> String {
        format!("{}\u{1f}{namespace}\u{1f}{id}", scope.as_str())
    }

    fn index_record_id(scope: MemoryScope) -> String {
        Self::record_id(scope, INDEX_NAMESPACE, INDEX_LOGICAL_ID)
    }

    fn record_type(scope: MemoryScope) -> RecordType {
        match scope {
            MemoryScope::Conversation => RecordType::Message,
            MemoryScope::Preferences => RecordType::Preference,
            MemoryScope::Project => RecordType::ContextSnapshot,
            MemoryScope::Document => RecordType::Document,
            MemoryScope::Relevant | MemoryScope::Extended => RecordType::Memory,
            MemoryScope::None => RecordType::Memory,
        }
    }

    fn encode(record: &MemoryRecord) -> HarnessResult<Vec<u8>> {
        let envelope = VaultMemoryEnvelope {
            schema: ENVELOPE_SCHEMA,
            harness_id: record.id.clone(),
            scope: record.scope.as_str().to_owned(),
            namespace: record.namespace.clone(),
            content: record.content.clone(),
            attributes: record.attributes.clone(),
        };
        serde_json::to_vec(&envelope).map_err(|error| {
            Failure::new(
                ErrorCode::Internal,
                FailureClass::Internal,
                "pai.memory.encode",
                format!("failed to encode memory envelope: {error}"),
            )
        })
    }

    fn decode(bytes: &[u8]) -> HarnessResult<MemoryRecord> {
        let envelope: VaultMemoryEnvelope = serde_json::from_slice(bytes).map_err(|_| {
            Failure::new(
                ErrorCode::ProviderFailed,
                FailureClass::Persistence,
                "pai.memory.decode",
                "encrypted vault record is not a supported Harness memory envelope",
            )
        })?;
        if envelope.schema != ENVELOPE_SCHEMA {
            return Err(Failure::new(
                ErrorCode::CapabilityUnavailable,
                FailureClass::Persistence,
                "pai.memory.decode",
                "unsupported Harness memory envelope schema",
            ));
        }
        let scope = parse_scope(&envelope.scope)?;
        let record = MemoryRecord {
            id: envelope.harness_id,
            scope,
            namespace: envelope.namespace,
            content: envelope.content,
            attributes: envelope.attributes,
        };
        record.validate()?;
        Ok(record)
    }

    fn read_index(
        &self,
        vault: &Vault,
        scope: MemoryScope,
    ) -> HarnessResult<(VaultMemoryIndex, bool)> {
        let id = Self::index_record_id(scope);
        match vault.read_record(&id) {
            Ok((_metadata, bytes)) => {
                if bytes.len() > MAX_INDEX_BYTES {
                    return Err(Failure::new(
                        ErrorCode::ProviderFailed,
                        FailureClass::Persistence,
                        "pai.memory.index.read",
                        "encrypted memory index exceeds hard size limit",
                    ));
                }
                let index: VaultMemoryIndex = serde_json::from_slice(&bytes).map_err(|_| {
                    Failure::new(
                        ErrorCode::ProviderFailed,
                        FailureClass::Persistence,
                        "pai.memory.index.read",
                        "encrypted memory index is malformed",
                    )
                })?;
                if index.schema != INDEX_SCHEMA
                    || index.scope != scope.as_str()
                    || index.entries.len() > MAX_INDEX_ENTRIES
                {
                    return Err(Failure::new(
                        ErrorCode::ProviderFailed,
                        FailureClass::Persistence,
                        "pai.memory.index.read",
                        "encrypted memory index schema, scope, or size is invalid",
                    ));
                }
                Ok((index, true))
            }
            Err(VaultError::VaultNotFound(_)) | Err(VaultError::NotPermitted(_)) => {
                Ok((VaultMemoryIndex::empty(scope), false))
            }
            Err(error) => Err(map_vault_error("pai.memory.index.read", error)),
        }
    }

    fn write_index(
        &self,
        vault: &mut Vault,
        scope: MemoryScope,
        index: &VaultMemoryIndex,
    ) -> HarnessResult<()> {
        if index.schema != INDEX_SCHEMA
            || index.scope != scope.as_str()
            || index.entries.len() > MAX_INDEX_ENTRIES
        {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Persistence,
                "pai.memory.index.write",
                "memory index exceeds supported bounds",
            ));
        }
        let bytes = serde_json::to_vec(index).map_err(|error| {
            Failure::new(
                ErrorCode::Internal,
                FailureClass::Internal,
                "pai.memory.index.write",
                format!("failed to serialize encrypted memory index: {error}"),
            )
        })?;
        if bytes.len() > MAX_INDEX_BYTES {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Persistence,
                "pai.memory.index.write",
                "memory index exceeds encrypted size limit",
            ));
        }

        let record_id = Self::index_record_id(scope);
        let mut metadata = match vault.read_record(&record_id) {
            Ok((metadata, _)) => metadata,
            Err(VaultError::VaultNotFound(_)) | Err(VaultError::NotPermitted(_)) => {
                let mut fresh = Record::new(
                    RecordType::ContextSnapshot,
                    &self.config.origin_platform,
                    &self.config.origin_device_id,
                );
                fresh.record_id = record_id;
                fresh
            }
            Err(error) => return Err(map_vault_error("pai.memory.index.write", error)),
        };
        metadata.record_type = RecordType::ContextSnapshot;
        metadata.privacy_level = PrivacyLevel::Private;
        metadata.origin_platform = self.config.origin_platform.clone();
        metadata.origin_device_id = self.config.origin_device_id.clone();
        vault
            .write_record(metadata, &bytes)
            .map_err(|error| map_vault_error("pai.memory.index.write", error))
    }

    fn read_logical(
        &self,
        scope: MemoryScope,
        namespace: &str,
        id: &str,
    ) -> HarnessResult<Option<(Record, MemoryRecord)>> {
        let record_id = Self::record_id(scope, namespace, id);
        self.with_vault("pai.memory.retrieve", |vault| {
            match vault.read_record(&record_id) {
                Ok((metadata, bytes)) => Ok(Some((metadata, Self::decode(&bytes)?))),
                Err(VaultError::VaultNotFound(_)) | Err(VaultError::NotPermitted(_)) => Ok(None),
                Err(error) => Err(map_vault_error("pai.memory.retrieve", error)),
            }
        })
    }

    fn write_logical(&self, record: MemoryRecord, update_only: bool) -> HarnessResult<()> {
        record.validate()?;
        let payload = Self::encode(&record)?;
        let record_id = Self::record_id(record.scope, &record.namespace, &record.id);
        self.with_vault_mut("pai.memory.store", |vault| {
            let existing = match vault.read_record(&record_id) {
                Ok((metadata, _)) => Some(metadata),
                Err(VaultError::VaultNotFound(_)) | Err(VaultError::NotPermitted(_)) => None,
                Err(error) => return Err(map_vault_error("pai.memory.store", error)),
            };
            if update_only && existing.is_none() {
                return Err(Failure::new(
                    ErrorCode::NotFound,
                    FailureClass::Persistence,
                    "pai.memory.update",
                    "memory record does not exist",
                ));
            }
            if !update_only && existing.is_some() {
                return Err(Failure::new(
                    ErrorCode::Conflict,
                    FailureClass::User,
                    "pai.memory.store",
                    "memory record already exists; use update for replacement",
                ));
            }

            // Index first while the vault mutex is held. If power is lost before
            // the data write, the stale index entry is self-healed on search.
            // This ordering makes a retry of store safe: no data record exists,
            // so the retry is not falsely rejected as a duplicate.
            let (mut index, _exists) = self.read_index(vault, record.scope)?;
            let key = Self::logical_key(record.scope, &record.namespace, &record.id);
            index.entries.insert(
                key,
                VaultMemoryIndexEntry {
                    data_record_id: record_id.clone(),
                    harness_id: record.id.clone(),
                    namespace: record.namespace.clone(),
                },
            );
            self.write_index(vault, record.scope, &index)?;

            let mut metadata = existing.unwrap_or_else(|| {
                let mut fresh = Record::new(
                    Self::record_type(record.scope),
                    &self.config.origin_platform,
                    &self.config.origin_device_id,
                );
                fresh.record_id = record_id.clone();
                fresh
            });
            metadata.record_type = Self::record_type(record.scope);
            metadata.privacy_level = PrivacyLevel::Private;
            metadata.origin_platform = self.config.origin_platform.clone();
            metadata.origin_device_id = self.config.origin_device_id.clone();

            vault
                .write_record(metadata, &payload)
                .map_err(|error| map_vault_error("pai.memory.store", error))
        })
    }

    fn rebuild_index(
        &self,
        vault: &mut Vault,
        scope: MemoryScope,
    ) -> HarnessResult<VaultMemoryIndex> {
        let root: PathBuf = vault.vault_root().join("VAULT").join("records");
        let mut paths = fs::read_dir(&root)
            .map_err(|error| {
                Failure::new(
                    ErrorCode::ProviderFailed,
                    FailureClass::Persistence,
                    "pai.memory.index.repair",
                    format!("cannot read encrypted vault record directory: {error}"),
                )
            })?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|x| x.to_str())
                    .is_some_and(|name| name.ends_with(".enc.json"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        if paths.len() > self.config.max_scan_records {
            return Err(Failure::new(
                ErrorCode::BudgetExceeded,
                FailureClass::Persistence,
                "pai.memory.index.repair",
                format!(
                    "memory index repair requires scanning {} records, above configured limit {}",
                    paths.len(),
                    self.config.max_scan_records
                ),
            ));
        }

        let mut index = VaultMemoryIndex::empty(scope);
        let index_record_id = Self::index_record_id(scope);
        for path in paths {
            let Some(name) = path.file_name().and_then(|x| x.to_str()) else {
                continue;
            };
            let Some(record_id) = name.strip_suffix(".enc.json") else {
                continue;
            };
            if record_id == index_record_id {
                continue;
            }

            // Read plaintext metadata first so unrelated vault domains do not
            // incur a decrypt attempt. Metadata is authenticated by vault-core
            // when the actual record is read below.
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(envelope) = serde_json::from_str::<EncryptedRecord>(&raw) else {
                continue;
            };
            if envelope.metadata.tombstone
                || envelope.metadata.record_type != Self::record_type(scope)
            {
                continue;
            }
            let Ok((_metadata, bytes)) = vault.read_record(record_id) else {
                continue;
            };
            let Ok(memory) = Self::decode(&bytes) else {
                continue;
            };
            if memory.scope != scope {
                continue;
            }
            if index.entries.len() >= MAX_INDEX_ENTRIES {
                return Err(Failure::new(
                    ErrorCode::BudgetExceeded,
                    FailureClass::Persistence,
                    "pai.memory.index.repair",
                    "memory index entry limit exceeded",
                ));
            }
            index.entries.insert(
                Self::logical_key(memory.scope, &memory.namespace, &memory.id),
                VaultMemoryIndexEntry {
                    data_record_id: record_id.to_owned(),
                    harness_id: memory.id,
                    namespace: memory.namespace,
                },
            );
        }
        self.write_index(vault, scope, &index)?;
        Ok(index)
    }

    fn indexed_candidates(&self, query: &MemoryQuery) -> HarnessResult<Vec<MemoryRecord>> {
        query.validate()?;
        self.with_vault_mut("pai.memory.search", |vault| {
            let (mut index, exists) = self.read_index(vault, query.scope)?;
            if !exists {
                index = self.rebuild_index(vault, query.scope)?;
            }

            let mut stale = Vec::new();
            let mut out = Vec::new();
            for (key, entry) in &index.entries {
                if query
                    .namespace
                    .as_ref()
                    .is_some_and(|namespace| namespace != &entry.namespace)
                {
                    continue;
                }
                match vault.read_record(&entry.data_record_id) {
                    Ok((_metadata, bytes)) => {
                        let Ok(memory) = Self::decode(&bytes) else {
                            stale.push(key.clone());
                            continue;
                        };
                        if memory.scope != query.scope
                            || memory.id != entry.harness_id
                            || memory.namespace != entry.namespace
                        {
                            stale.push(key.clone());
                            continue;
                        }
                        out.push(memory);
                    }
                    Err(VaultError::VaultNotFound(_)) | Err(VaultError::NotPermitted(_)) => {
                        stale.push(key.clone());
                    }
                    Err(error) => return Err(map_vault_error("pai.memory.search", error)),
                }
            }

            if !stale.is_empty() {
                for key in stale {
                    index.entries.remove(&key);
                }
                self.write_index(vault, query.scope, &index)?;
            }
            Ok(out)
        })
    }
}

impl MemoryProvider for PaiVaultMemoryProvider {
    fn capabilities(&self) -> MemoryCapabilities {
        MemoryCapabilities {
            scopes: vec![
                MemoryScope::Conversation,
                MemoryScope::Preferences,
                MemoryScope::Relevant,
                MemoryScope::Project,
                MemoryScope::Document,
                MemoryScope::Extended,
            ],
            can_retrieve: true,
            can_search: true,
            can_store: true,
            can_update: true,
            can_delete: true,
            max_results: 64,
        }
    }

    fn retrieve(
        &self,
        scope: MemoryScope,
        namespace: &str,
        id: &str,
    ) -> HarnessResult<Option<MemoryRecord>> {
        if scope == MemoryScope::None
            || namespace.is_empty()
            || namespace.len() > 256
            || namespace.contains('\0')
            || !valid_memory_id(id)
        {
            return Err(Failure::invalid(
                "pai.memory.retrieve",
                "invalid scope, namespace, or id",
            ));
        }
        self.read_logical(scope, namespace, id)
            .map(|item| item.map(|(_, memory)| memory))
    }

    fn search(&self, query: &MemoryQuery) -> HarnessResult<Vec<MemoryRecord>> {
        let candidates = self.indexed_candidates(query)?;
        let terms = tokenize(&query.text);
        let mut ranked = candidates
            .into_iter()
            .map(|record| {
                let mut searchable = record.id.to_ascii_lowercase();
                searchable.push(' ');
                searchable.push_str(&record.content.to_ascii_lowercase());
                for (key, value) in &record.attributes {
                    searchable.push(' ');
                    searchable.push_str(&key.to_ascii_lowercase());
                    searchable.push(' ');
                    searchable.push_str(&value.to_ascii_lowercase());
                }
                let score = if terms.is_empty() {
                    0usize
                } else {
                    terms
                        .iter()
                        .filter(|term| searchable.contains(term.as_str()))
                        .count()
                };
                (score, record)
            })
            .collect::<Vec<_>>();
        if terms.is_empty() {
            // Empty-text search is the universal Harness contract for an exact
            // recent conversation window. Select newest first, then return the
            // bounded window in chronological order for model readability.
            ranked.sort_by_key(|(_, record)| Reverse(conversation_order_key(record)));
            let mut recent = ranked
                .into_iter()
                .map(|(_, record)| record)
                .take(query.limit.min(256))
                .collect::<Vec<_>>();
            recent.sort_by_key(conversation_order_key);
            Ok(recent)
        } else {
            ranked.sort_by_key(|(score, record)| (Reverse(*score), record.id.clone()));
            let mut hits: Vec<MemoryRecord> = ranked
                .into_iter()
                .filter(|(score, _)| *score > 0)
                .map(|(_, record)| record)
                .take(query.limit.min(64))
                .collect();
            // Gap 5: after the lexical filter, one bounded model call
            // reorders the hits by real relevance. Fail-open — the lexical
            // order above is returned untouched on any rerank failure.
            self.rerank_lexical_hits(&query.text, &mut hits);
            Ok(hits)
        }
    }

    fn store(&self, record: MemoryRecord) -> HarnessResult<()> {
        self.write_logical(record, false)
    }

    fn update(&self, record: MemoryRecord) -> HarnessResult<()> {
        self.write_logical(record, true)
    }

    fn delete(&self, scope: MemoryScope, namespace: &str, id: &str) -> HarnessResult<bool> {
        if self.retrieve(scope, namespace, id)?.is_none() {
            return Ok(false);
        }
        let record_id = Self::record_id(scope, namespace, id);
        self.with_vault_mut("pai.memory.delete", |vault| {
            vault
                .delete_record(
                    &record_id,
                    &self.config.origin_platform,
                    &self.config.origin_device_id,
                )
                .map_err(|error| map_vault_error("pai.memory.delete", error))?;

            // Tombstone first. If power is lost before the index update, search
            // sees the stale entry and removes it automatically on next use.
            let (mut index, exists) = self.read_index(vault, scope)?;
            if exists {
                index
                    .entries
                    .remove(&Self::logical_key(scope, namespace, id));
                self.write_index(vault, scope, &index)?;
            }
            Ok(())
        })?;
        Ok(true)
    }
}

fn conversation_order_key(record: &MemoryRecord) -> (u128, u32, String) {
    let timestamp = record
        .attributes
        .get("created_at_ms")
        .and_then(|value| value.parse::<u128>().ok())
        .unwrap_or(0);
    let role_order = record
        .attributes
        .get("role_order")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0);
    (timestamp, role_order, record.id.clone())
}

fn valid_memory_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn parse_scope(scope: &str) -> HarnessResult<MemoryScope> {
    match scope {
        "conversation" => Ok(MemoryScope::Conversation),
        "preferences" => Ok(MemoryScope::Preferences),
        "relevant" => Ok(MemoryScope::Relevant),
        "project" => Ok(MemoryScope::Project),
        "document" => Ok(MemoryScope::Document),
        "extended" => Ok(MemoryScope::Extended),
        _ => Err(Failure::new(
            ErrorCode::CapabilityUnavailable,
            FailureClass::Persistence,
            "pai.memory.scope",
            "unsupported memory scope in vault record",
        )),
    }
}

fn tokenize(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|term| term.chars().count() >= 2)
        .take(64)
        .map(|term| term.to_lowercase())
        .collect()
}

fn map_vault_error(operation: &'static str, error: VaultError) -> Failure {
    let (code, class, message) = match error {
        VaultError::VaultLocked => (
            ErrorCode::PermissionDenied,
            FailureClass::Persistence,
            "Pocket AI vault is locked".to_owned(),
        ),
        VaultError::VaultNotFound(message) => {
            (ErrorCode::NotFound, FailureClass::Persistence, message)
        }
        VaultError::NotPermitted(message) => {
            (ErrorCode::PermissionDenied, FailureClass::Policy, message)
        }
        other => (
            ErrorCode::ProviderFailed,
            FailureClass::Persistence,
            other.to_string(),
        ),
    };
    Failure::new(code, class, operation, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use inbharat_harness_core::MemoryProvider;

    fn provider() -> (tempfile::TempDir, PaiVaultMemoryProvider) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("UNOONE");
        Vault::create(&root, b"test-password-12345").unwrap();
        let mut vault = Vault::open(&root).unwrap();
        vault.unlock(b"test-password-12345").unwrap();
        let provider = PaiVaultMemoryProvider::new(
            Arc::new(Mutex::new(Some(vault))),
            PaiVaultMemoryProviderConfig {
                origin_device_id: "test-device".to_owned(),
                ..Default::default()
            },
        )
        .unwrap();
        (temp, provider)
    }

    fn memory(id: &str, scope: MemoryScope, namespace: &str, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_owned(),
            scope,
            namespace: namespace.to_owned(),
            content: content.to_owned(),
            attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn round_trip_is_encrypted_vault_backed() {
        let (temp, provider) = provider();
        provider
            .store(memory(
                "preference-1",
                MemoryScope::Preferences,
                "user-1",
                "prefers offline execution",
            ))
            .unwrap();
        let got = provider
            .retrieve(MemoryScope::Preferences, "user-1", "preference-1")
            .unwrap()
            .unwrap();
        assert_eq!(got.content, "prefers offline execution");

        let records = temp.path().join("UNOONE").join("VAULT").join("records");
        for entry in std::fs::read_dir(records).unwrap().flatten() {
            let raw = std::fs::read(entry.path()).unwrap();
            assert!(
                !String::from_utf8_lossy(&raw).contains("prefers offline execution"),
                "memory plaintext leaked into an encrypted vault record"
            );
        }
    }

    #[test]
    fn store_is_create_only_and_update_is_explicit() {
        let (_temp, provider) = provider();
        let first = memory("memory-1", MemoryScope::Relevant, "vault-a", "first value");
        provider.store(first.clone()).unwrap();
        let duplicate = provider.store(first.clone()).unwrap_err();
        assert_eq!(duplicate.code, ErrorCode::Conflict);

        let mut updated = first;
        updated.content = "updated value".to_owned();
        provider.update(updated).unwrap();
        assert_eq!(
            provider
                .retrieve(MemoryScope::Relevant, "vault-a", "memory-1")
                .unwrap()
                .unwrap()
                .content,
            "updated value"
        );
    }

    #[test]
    fn namespace_isolation_and_indexed_search_are_strict() {
        let (_temp, provider) = provider();
        provider
            .store(memory(
                "memory-1",
                MemoryScope::Relevant,
                "vault-a",
                "private cardamom context",
            ))
            .unwrap();
        provider
            .store(memory(
                "memory-2",
                MemoryScope::Relevant,
                "vault-b",
                "private cardamom context",
            ))
            .unwrap();

        assert!(provider
            .retrieve(MemoryScope::Relevant, "vault-b", "memory-1")
            .unwrap()
            .is_none());
        let found = provider
            .search(&MemoryQuery {
                scope: MemoryScope::Relevant,
                namespace: Some("vault-a".to_owned()),
                text: "cardamom".to_owned(),
                limit: 10,
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "memory-1");
    }

    #[test]
    fn delete_tombstones_data_and_removes_search_entry() {
        let (_temp, provider) = provider();
        provider
            .store(memory(
                "memory-1",
                MemoryScope::Relevant,
                "vault-a",
                "delete me",
            ))
            .unwrap();
        assert!(provider
            .delete(MemoryScope::Relevant, "vault-a", "memory-1")
            .unwrap());
        assert!(provider
            .retrieve(MemoryScope::Relevant, "vault-a", "memory-1")
            .unwrap()
            .is_none());
        assert!(provider
            .search(&MemoryQuery {
                scope: MemoryScope::Relevant,
                namespace: Some("vault-a".to_owned()),
                text: "delete".to_owned(),
                limit: 10,
            })
            .unwrap()
            .is_empty());
    }

    /// Gap 5 fail-open contract: the rerank is a quality boost layered on the
    /// lexical search, never a dependency. A dead model endpoint (or a model
    /// that answers garbage) must leave the proven lexical order untouched,
    /// and search must still succeed.
    #[test]
    fn rerank_failure_keeps_lexical_order_and_search_succeeds() {
        let (_temp, provider) = provider();
        // A port with no listener: the rerank call fails, the lexical hits
        // still come back. Port 1 is reserved and never a llama-server.
        let provider = provider.with_lexical_rerank("test-model", 1);
        for (id, content) in [
            ("memory-1", "alpha cardamom notes"),
            ("memory-2", "beta cardamom notes"),
            ("memory-3", "gamma cardamom notes"),
        ] {
            provider
                .store(memory(id, MemoryScope::Relevant, "vault-a", content))
                .unwrap();
        }
        let found = provider
            .search(&MemoryQuery {
                scope: MemoryScope::Relevant,
                namespace: Some("vault-a".to_owned()),
                text: "cardamom".to_owned(),
                limit: 10,
            })
            .unwrap();
        assert_eq!(found.len(), 3);
        let ids: Vec<&str> = found.iter().map(|record| record.id.as_str()).collect();
        assert_eq!(ids, ["memory-1", "memory-2", "memory-3"]);
    }

    /// A disabled rerank (empty model id, or port 0) is a plain no-op, and
    /// below three hits the model is never called even when configured.
    #[test]
    fn rerank_is_noop_when_disabled_or_below_threshold() {
        let (_temp, provider) = provider();
        let provider = provider
            .with_lexical_rerank("", 8080)
            .with_lexical_rerank("m", 0);
        provider
            .store(memory(
                "memory-1",
                MemoryScope::Relevant,
                "vault-a",
                "alpha cardamom notes",
            ))
            .unwrap();
        let found = provider
            .search(&MemoryQuery {
                scope: MemoryScope::Relevant,
                namespace: Some("vault-a".to_owned()),
                text: "cardamom".to_owned(),
                limit: 10,
            })
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "memory-1");
    }
}
