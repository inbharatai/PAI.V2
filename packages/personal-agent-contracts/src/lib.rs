//! Inert wire records, not execution or authorization. Use `decode` at every native
//! input boundary; serde on an individual DTO is NOT the validated admission API.
//! No network, storage, cryptography, provider or model dependency exists here.
mod types;
pub use types::*;
pub mod authority;
pub mod fold;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const SCHEMA: &str = "inbharat.pai.personal-agent";
pub const MAX_BYTES: usize = 65_536;
pub const MAX_DEPTH: usize = 12;
pub const MAX_ITEMS: usize = 64;
pub const MAX_STRING_BYTES: usize = 4096;
pub type Result<T> = std::result::Result<T, String>;
fn required_nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Document {
    pub schema: String,
    pub version: Version,
    #[serde(flatten)]
    pub record: Record,
}
impl Document {
    pub fn new(record: Record) -> Result<Self> {
        let doc = Self {
            schema: SCHEMA.into(),
            version: Version { major: 1, minor: 0 },
            record,
        };
        doc.validate()?;
        Ok(doc)
    }
    pub fn validate(&self) -> Result<()> {
        ensure(
            self.schema == SCHEMA && self.version == Version { major: 1, minor: 0 },
            "unsupported schema/version",
        )?;
        let value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        bounds(&value, 0, "")?;
        ensure(
            serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() <= MAX_BYTES,
            "document byte limit",
        )?;
        validate_record(&self.record)
    }
    /// Hydration is data-only, regardless of record kind, approval or receipt claim.
    pub fn may_execute_on_hydration(&self) -> bool {
        false
    }
}
pub(crate) fn ensure(ok: bool, reason: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(reason.into())
    }
}

/// Strict v1 reader. Unknown major/minor/fields fail without a mutation.
/// Existing capability.v1/vault readers stay separate; this creates no v2 migration.
pub fn decode(input: &[u8]) -> Result<Document> {
    preflight(input)?;
    let StrictValue(value) = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    bounds(&value, 0, "")?;
    let mut object = value
        .as_object()
        .cloned()
        .ok_or("envelope must be object")?;
    ensure(object.len() == 4, "unknown/missing envelope field")?;
    let schema: String = serde_json::from_value(object.remove("schema").ok_or("missing schema")?)
        .map_err(|e| e.to_string())?;
    let version: Version =
        serde_json::from_value(object.remove("version").ok_or("missing version")?)
            .map_err(|e| e.to_string())?;
    ensure(
        schema == SCHEMA && version == Version { major: 1, minor: 0 },
        "unsupported schema/version",
    )?;
    let record = serde_json::from_value(Value::Object(object)).map_err(|e| e.to_string())?;
    let doc = Document {
        schema,
        version,
        record,
    };
    doc.validate()?;
    Ok(doc)
}
pub fn encode(doc: &Document) -> Result<Vec<u8>> {
    doc.validate()?;
    let bytes = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
    ensure(bytes.len() <= MAX_BYTES, "document byte limit")?;
    Ok(bytes)
}
fn preflight(input: &[u8]) -> Result<()> {
    ensure(input.len() <= MAX_BYTES, "document byte limit")?;
    let (mut depth, mut string, mut escape) = (0usize, false, false);
    for &b in input {
        if string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                string = false;
            }
        } else {
            match b {
                b'"' => string = true,
                b'{' | b'[' => {
                    depth += 1;
                    ensure(depth <= MAX_DEPTH, "nesting limit")?;
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}
fn bounds(value: &Value, depth: usize, key: &str) -> Result<()> {
    ensure(depth <= MAX_DEPTH, "nesting limit")?;
    match value {
        Value::String(s) => {
            ensure(s.len() <= MAX_STRING_BYTES, "string byte limit")?;
            if key.ends_with("_id") || key == "idempotency_key" {
                ensure(
                    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_graphic()),
                    "invalid identifier",
                )?;
            }
        }
        Value::Array(items) => {
            ensure(items.len() <= MAX_ITEMS, "array limit")?;
            for item in items {
                bounds(item, depth + 1, key)?;
            }
        }
        Value::Object(map) => {
            ensure(map.len() <= MAX_ITEMS, "object limit")?;
            for (k, v) in map {
                bounds(v, depth + 1, k)?;
            }
        }
        Value::Number(n) => ensure(
            n.as_u64().is_some_and(|x| x <= 9_007_199_254_740_991),
            "integer range",
        )?,
        _ => {}
    }
    Ok(())
}
impl Budget {
    pub fn validate(&self) -> Result<()> {
        ensure(
            self.max_steps <= 1024
                && self.max_tool_calls <= 1024
                && self.max_duration_ms > 0
                && self.max_duration_ms <= 86_400_000
                && self.max_bytes <= 67_108_864
                && self.max_network_calls <= 1024
                && self.max_depth <= 2
                && self.max_children <= 2,
            "budget bounds",
        )
    }
    pub fn intersect(&self, other: &Self) -> Self {
        Self {
            max_steps: self.max_steps.min(other.max_steps),
            max_tool_calls: self.max_tool_calls.min(other.max_tool_calls),
            max_duration_ms: self.max_duration_ms.min(other.max_duration_ms),
            max_bytes: self.max_bytes.min(other.max_bytes),
            max_network_calls: self.max_network_calls.min(other.max_network_calls),
            max_depth: self.max_depth.min(other.max_depth),
            max_children: self.max_children.min(other.max_children),
        }
    }
}
fn unique<T: Ord>(items: &[T]) -> bool {
    items.iter().collect::<BTreeSet<_>>().len() == items.len()
}
impl Scopes {
    pub fn validate(&self) -> Result<()> {
        ensure(
            unique(&self.capabilities)
                && unique(&self.tools)
                && unique(&self.recipients)
                && unique(&self.hosts)
                && unique(&self.operations),
            "duplicate scope",
        )?;
        for list in [
            &self.capabilities,
            &self.tools,
            &self.recipients,
            &self.hosts,
        ] {
            ensure(
                list.len() <= MAX_ITEMS
                    && list.iter().all(|s| {
                        !s.is_empty()
                            && s.len() <= 256
                            && s.bytes().all(|b| b.is_ascii_graphic())
                            && !s.contains('*')
                    }),
                "exact bounded scopes required",
            )?;
        }
        ensure(self.data.len() <= MAX_ITEMS, "data scopes limit")?;
        let mut seen = BTreeSet::new();
        for d in &self.data {
            ensure(
                !d.resource_id.is_empty()
                    && !d.resource_id.contains('*')
                    && d.resource_id.len() <= 128
                    && d.resource_id.bytes().all(|b| b.is_ascii_graphic())
                    && unique(&d.operations)
                    && seen.insert((d.kind, &d.resource_id)),
                "invalid data scope",
            )?;
            ensure(
                d.operations.iter().all(|o| self.operations.contains(o)),
                "data operation outside scope",
            )?;
        }
        ensure(
            self.delegation != DelegationLevel::ReadAndSuggest
                || self
                    .operations
                    .iter()
                    .all(|o| matches!(o, Operation::Read | Operation::Suggest)),
            "read-only delegation",
        )?;
        ensure(
            self.delegation != DelegationLevel::PrepareDrafts
                || self
                    .operations
                    .iter()
                    .all(|o| matches!(o, Operation::Read | Operation::Suggest | Operation::Draft)),
            "draft-only delegation",
        )
    }
    /// Exact set intersection, never path-prefix, wildcard or email-domain expansion.
    pub fn intersect(&self, other: &Self) -> Self {
        fn both<T: Ord + Clone>(a: &[T], b: &[T]) -> Vec<T> {
            a.iter()
                .filter(|x| b.contains(x))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        }
        let mut data = Vec::new();
        for a in &self.data {
            if let Some(b) = other
                .data
                .iter()
                .find(|b| b.kind == a.kind && b.resource_id == a.resource_id)
            {
                data.push(DataScope {
                    kind: a.kind,
                    resource_id: a.resource_id.clone(),
                    operations: both(&a.operations, &b.operations),
                });
            }
        }
        data.sort_by(|a, b| (a.kind, &a.resource_id).cmp(&(b.kind, &b.resource_id)));
        Self {
            capabilities: both(&self.capabilities, &other.capabilities),
            tools: both(&self.tools, &other.tools),
            data,
            recipients: both(&self.recipients, &other.recipients),
            hosts: both(&self.hosts, &other.hosts),
            operations: both(&self.operations, &other.operations),
            delegation: self.delegation.min(other.delegation),
            network: self.network.min(other.network),
        }
    }
}
fn window(start: u64, end: u64) -> Result<()> {
    ensure(
        end > start && end - start <= 86_400_000,
        "deadline/expiry must be within 24 hours",
    )
}
fn scope_budget(s: &Scopes, b: &Budget) -> Result<()> {
    s.validate()?;
    b.validate()?;
    ensure(
        s.network != NetworkPolicy::OfflineOnly || b.max_network_calls == 0,
        "offline network budget",
    )
}
fn validate_record(r: &Record) -> Result<()> {
    match r {
        Record::Persona(x) => {
            ensure(
                x.corrects_revision.is_none_or(|v| v < x.revision),
                "correction must precede revision",
            )?;
            ensure(
                !x.deleted || x.preferences.is_empty(),
                "deleted persona retains no preferences",
            )?;
            for p in &x.preferences {
                ensure(
                    p.status == PreferenceStatus::Observed
                        || p.provenance.source == ProvenanceSource::User,
                    "preference approval/correction requires user provenance claim",
                )?;
            }
        }
        Record::TaskSpec(x) => {
            scope_budget(&x.scopes, &x.budget)?;
            window(x.created_at_ms, x.deadline_ms)?;
            ensure(
                x.budget.max_duration_ms <= x.deadline_ms - x.created_at_ms
                    && !x.goal.trim().is_empty()
                    && !x.expected_postcondition.trim().is_empty(),
                "task bounds/postcondition",
            )?;
        }
        Record::TaskEvent(x) => {
            ensure(
                x.step <= 1024
                    && x.deadline_ms > 0
                    && x.predecessor_event_id.as_ref() != Some(&x.event_id),
                "invalid event",
            )?;
            ensure(
                x.transition != TaskTransition::Verified
                    || x.evidence_ref.as_ref().is_some_and(|s| !s.is_empty()),
                "verified claim requires evidence reference",
            )?;
        }
        Record::AgentSpec(x) => {
            scope_budget(&x.scopes, &x.budget)?;
            ensure(
                x.depth > 0
                    && x.depth <= x.budget.max_depth
                    && x.template_version > 0
                    && x.expires_at_ms > 0,
                "agent depth/expiry/version",
            )?;
        }
        Record::TaskReceipt(x) => {
            ensure(
                !matches!(
                    x.outcome,
                    ReceiptOutcome::Verified | ReceiptOutcome::ActionVerified
                ) || !x.after_evidence_refs.is_empty(),
                "receipt evidence required",
            )?;
            ensure(
                x.outcome != ReceiptOutcome::Verified
                    || !matches!(
                        x.dispatch_intent,
                        DispatchIntent::OpenComposer | DispatchIntent::OpenEventForm
                    ),
                "opening UI cannot verify goal",
            )?;
        }
        Record::Handoff(x) => {
            ensure(
                x.from_replica_id != x.target_replica_id
                    && !x.encrypted_goal.is_empty()
                    && x.expires_at_ms > 0,
                "handoff target/payload/expiry",
            )?;
        }
        Record::ReplicaChange(x) => {
            ensure(
                x.sequence > 0 && x.record_revision > 0 && x.provenance.replica_id == x.replica_id,
                "change sequence/provenance",
            )?;
            ensure(
                x.content_hash.len() == 64
                    && x.content_hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                "content hash syntax",
            )?;
            ensure(
                match x.change {
                    ChangeKind::Tombstone => x.ciphertext.is_none(),
                    ChangeKind::Upsert => x.ciphertext.as_ref().is_some_and(|s| !s.is_empty()),
                },
                "ciphertext/tombstone mismatch",
            )?;
            ensure(
                x.record_kind != RecordKind::CapabilityGrant,
                "grants cannot be replicated",
            )?;
        }
        Record::Draft(x) => {
            ensure(
                x.reply_to
                    .as_ref()
                    .is_none_or(|m| m.account_id == x.account_id),
                "reply account mismatch",
            )?;
        }
        Record::EventRef(x) => {
            ensure(
                x.end_ms > x.start_ms
                    && !x.time_zone.is_empty()
                    && x.time_zone == x.calendar.time_zone,
                "event time/calendar mismatch",
            )?;
        }
        Record::CalendarRef(x) => ensure(!x.time_zone.is_empty(), "time zone required")?,
        Record::CapabilityGrant(x) => {
            scope_budget(&x.scopes, &x.budget)?;
            window(x.issued_at_ms, x.expires_at_ms)?;
            ensure(
                x.budget.max_duration_ms <= x.expires_at_ms - x.issued_at_ms,
                "grant duration exceeds expiry",
            )?;
        }
        _ => {}
    }
    Ok(())
}

// Reject duplicate keys instead of letting a JSON Value silently use last-writer-wins.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bounded JSON")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Err(E::custom("integer required"))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_string<E: serde::de::Error>(
                self,
                v: String,
            ) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut v = Vec::new();
                while let Some(StrictValue(x)) = a.next_element()? {
                    v.push(x);
                    if v.len() > MAX_ITEMS {
                        return Err(serde::de::Error::custom("array limit"));
                    }
                }
                Ok(StrictValue(Value::Array(v)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut v = serde_json::Map::new();
                while let Some((k, StrictValue(x))) = a.next_entry::<String, StrictValue>()? {
                    if v.insert(k, x).is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    if v.len() > MAX_ITEMS {
                        return Err(serde::de::Error::custom("object limit"));
                    }
                }
                Ok(StrictValue(Value::Object(v)))
            }
        }
        d.deserialize_any(Visitor)
    }
}
