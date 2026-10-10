//! Optional external connectors: declared manifest, consent preview, revocation and the
//! outbound egress policy. Default is OFFLINE: with no consented manifest, every egress is refused.
//! No connector is enabled by default and there is no cloud fallback path in this module.

use crate::fnv64;
use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA: &str = "inbharat.pai.connector-manifest";
pub const MANIFEST_VERSION: u32 = 1;

/// Reviewed declaration of everything a connector may do. Shipped with the product, not
/// produced by a model or downloaded. Every field is user-visible in the consent preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorManifest {
    pub schema: String,
    pub manifest_version: u32,
    pub connector_id: String,
    pub provider: String,
    pub purpose: String,
    /// Exact https hosts; a request to any other host is refused.
    pub endpoints: Vec<String>,
    pub permitted_operations: Vec<String>,
    /// Account identifiers (e.g. mailbox address) the connector may act for; empty = none yet.
    pub accounts: Vec<String>,
    /// Exact data fields that may leave the device.
    pub data_fields: Vec<String>,
    pub token_scopes: Vec<String>,
    pub retention: String,
    pub expected_cost: String,
    pub max_request_bytes: u64,
    pub max_requests_per_day: u32,
    pub expires_at_ms: u64,
    pub revocation: String,
}

impl ConnectorManifest {
    pub fn validate(&self) -> Result<(), String> {
        let ok = |b: bool, m: &str| if b { Ok(()) } else { Err(m.to_string()) };
        ok(
            self.schema == MANIFEST_SCHEMA && self.manifest_version == MANIFEST_VERSION,
            "Unknown manifest schema/version",
        )?;
        ok(
            !self.connector_id.trim().is_empty()
                && self.connector_id.len() <= 64
                && self
                    .connector_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
            "Invalid connector id",
        )?;
        ok(
            !self.provider.trim().is_empty() && self.provider.len() <= 128,
            "Provider required",
        )?;
        ok(
            !self.purpose.trim().is_empty() && self.purpose.len() <= 512,
            "Purpose required",
        )?;
        ok(
            !self.endpoints.is_empty() && self.endpoints.len() <= 16,
            "1–16 exact endpoints required",
        )?;
        for e in &self.endpoints {
            ok(
                crate::domain::normalize_host(e).is_some()
                    && !e.contains('/')
                    && !e.contains('*')
                    && !crate::domain::is_ip_literal(e),
                "Endpoints must be exact https host names",
            )?;
        }
        ok(
            !self.permitted_operations.is_empty() && self.permitted_operations.len() <= 32,
            "Permitted operations required",
        )?;
        ok(
            !self.data_fields.is_empty() && self.data_fields.len() <= 64,
            "Data fields must be listed",
        )?;
        ok(
            self.accounts.len() <= 8 && self.token_scopes.len() <= 32,
            "Too many accounts/scopes",
        )?;
        ok(
            !self.retention.trim().is_empty()
                && !self.expected_cost.trim().is_empty()
                && !self.revocation.trim().is_empty(),
            "Retention, cost and revocation text required",
        )?;
        ok(
            self.max_request_bytes > 0 && self.max_request_bytes <= 64 * 1024 * 1024,
            "Request byte ceiling out of range",
        )?;
        ok(
            self.max_requests_per_day > 0 && self.max_requests_per_day <= 100_000,
            "Daily request ceiling out of range",
        )?;
        ok(self.expires_at_ms > 0, "Expiry required")?;
        Ok(())
    }
    pub fn digest(&self) -> String {
        format!(
            "{:016x}",
            fnv64(serde_json::to_string(self).unwrap_or_default().as_bytes())
        )
    }
    /// Scope breadth: broad write/delete/export operations or many data fields are flagged.
    pub fn broad_scope_reasons(&self) -> Vec<String> {
        let mut r = vec![];
        let broad = [
            "send",
            "delete",
            "export",
            "write_all",
            "full_access",
            "*",
            "all",
        ];
        for op in &self.permitted_operations {
            if broad.iter().any(|b| op.to_lowercase().contains(b)) {
                r.push(format!("operation '{op}' can cause external effects"));
            }
        }
        if self.data_fields.len() > 12
            || self
                .data_fields
                .iter()
                .any(|f| f.contains('*') || f.to_lowercase() == "all")
        {
            r.push("broad data field scope".into());
        }
        if self.token_scopes.iter().any(|s| {
            s.contains("mail.google.com")
                || s.ends_with("/gmail.modify")
                || s.ends_with("/gmail.compose")
                || s.contains("full_access")
        }) {
            r.push("write-capable token scope".into());
        }
        if self.max_request_bytes > 8 * 1024 * 1024 {
            r.push("large per-request byte ceiling".into());
        }
        if self.retention.to_lowercase().contains("unknown")
            || self.retention.to_lowercase().contains("indefinite")
        {
            r.push("provider retention is unknown/indefinite".into());
        }
        r
    }
    /// Short consent text; the UI shows this before the opt-in control.
    pub fn consent_preview(&self) -> String {
        format!(
            "{} will be sent to {} ({}) to {}. Operations: {}. Accounts: {}. Token scopes: {}. Retention: {}. Expected cost: {}. Limits: {} bytes/request, {} requests/day. Expires: {} (ms). Revoke: {}.",
            self.data_fields.join(", "),
            self.provider,
            self.endpoints.join(", "),
            self.purpose,
            self.permitted_operations.join(", "),
            if self.accounts.is_empty() { "none selected".to_string() } else { self.accounts.join(", ") },
            if self.token_scopes.is_empty() { "none".to_string() } else { self.token_scopes.join(", ") },
            self.retention,
            self.expected_cost,
            self.max_request_bytes,
            self.max_requests_per_day,
            self.expires_at_ms,
            self.revocation
        )
    }
}

/// Explicit scoped opt-in bound to the exact manifest digest. Constructed only by a native
/// consent UI after showing `consent_preview()`; not deserializable from a model or peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectorConsent {
    pub connector_id: String,
    pub manifest_digest: String,
    pub granted_at_ms: u64,
    pub expires_at_ms: u64,
    pub revoked_at_ms: Option<u64>,
}

impl ConnectorConsent {
    pub fn grant(manifest: &ConnectorManifest, now: u64) -> Result<Self, String> {
        manifest.validate()?;
        if manifest.expires_at_ms <= now {
            return Err("Manifest already expired".into());
        }
        Ok(Self {
            connector_id: manifest.connector_id.clone(),
            manifest_digest: manifest.digest(),
            granted_at_ms: now,
            expires_at_ms: manifest.expires_at_ms,
            revoked_at_ms: None,
        })
    }
    pub fn active(&self, manifest: &ConnectorManifest, now: u64) -> bool {
        self.revoked_at_ms.is_none()
            && self.connector_id == manifest.connector_id
            && self.manifest_digest == manifest.digest()
            && now < self.expires_at_ms
            && now < manifest.expires_at_ms
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EgressReceipt {
    pub connector_id: String,
    pub host: String,
    pub bytes: u64,
    pub at_ms: u64,
}

/// Host-owned outbound policy. Default offline. The model never sees or influences this object.
#[derive(Debug, Default, Clone)]
pub struct EgressPolicy {
    consents: Vec<ConnectorConsent>,
    /// (connector_id, day_index, count) — bounded in-memory daily counter.
    counters: Vec<(String, u64, u32)>,
    pub audit: Vec<EgressReceipt>,
}

impl EgressPolicy {
    pub fn offline() -> Self {
        Self::default()
    }
    pub fn consent(&mut self, consent: ConnectorConsent) {
        self.consents
            .retain(|c| c.connector_id != consent.connector_id);
        self.consents.push(consent);
    }
    /// Revocation stops new requests immediately. Token erasure is the caller's credential store duty
    /// and is reported, not assumed; provider-side deletion is never promised here.
    pub fn revoke(&mut self, connector_id: &str, now: u64) -> bool {
        let mut hit = false;
        for c in &mut self.consents {
            if c.connector_id == connector_id && c.revoked_at_ms.is_none() {
                c.revoked_at_ms = Some(now);
                hit = true;
            }
        }
        hit
    }
    pub fn is_offline(&self, manifests: &[ConnectorManifest], now: u64) -> bool {
        !self
            .consents
            .iter()
            .any(|c| manifests.iter().any(|m| c.active(m, now)))
    }
    /// Every outbound request must pass here with its exact URL and byte size.
    pub fn authorize(
        &mut self,
        manifests: &[ConnectorManifest],
        url: &str,
        bytes: u64,
        now: u64,
    ) -> Result<EgressReceipt, String> {
        let parsed = url::Url::parse(url).map_err(|_| "Egress refused: unparseable destination")?;
        if parsed.scheme() != "https" {
            return Err("Egress refused: only https connector endpoints".into());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Egress refused: credentials in URL".into());
        }
        let host = parsed
            .host_str()
            .and_then(crate::domain::normalize_host)
            .ok_or("Egress refused: no host")?;
        for manifest in manifests {
            if manifest.validate().is_err() {
                continue;
            }
            let Some(consent) = self
                .consents
                .iter()
                .find(|c| c.connector_id == manifest.connector_id)
            else {
                continue;
            };
            if !consent.active(manifest, now) {
                continue;
            }
            if !manifest
                .endpoints
                .iter()
                .any(|e| crate::domain::normalize_host(e).as_deref() == Some(host.as_str()))
            {
                continue;
            }
            if bytes > manifest.max_request_bytes {
                return Err(format!(
                    "Egress refused: {bytes} bytes exceeds connector ceiling {}",
                    manifest.max_request_bytes
                ));
            }
            let day = now / 86_400_000;
            let counter = match self
                .counters
                .iter_mut()
                .find(|(id, d, _)| *id == manifest.connector_id && *d == day)
            {
                Some(c) => c,
                None => {
                    self.counters.retain(|(_, d, _)| *d == day);
                    self.counters.push((manifest.connector_id.clone(), day, 0));
                    self.counters.last_mut().unwrap()
                }
            };
            if counter.2 >= manifest.max_requests_per_day {
                return Err("Egress refused: connector daily request ceiling reached".into());
            }
            counter.2 += 1;
            let receipt = EgressReceipt {
                connector_id: manifest.connector_id.clone(),
                host,
                bytes,
                at_ms: now,
            };
            if self.audit.len() >= 512 {
                self.audit.remove(0);
            }
            self.audit.push(receipt.clone());
            return Ok(receipt);
        }
        Err(format!(
            "Egress refused: no consented connector declares host {host}; device stays offline"
        ))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn manifest() -> ConnectorManifest {
        ConnectorManifest {
            schema: MANIFEST_SCHEMA.into(),
            manifest_version: 1,
            connector_id: "test-stub".into(),
            provider: "Stub Provider".into(),
            purpose: "run one fixture task".into(),
            endpoints: vec!["stub.example.com".into()],
            permitted_operations: vec!["read".into()],
            accounts: vec!["fixture@example.com".into()],
            data_fields: vec!["subject".into(), "body".into()],
            token_scopes: vec![],
            retention: "none claimed".into(),
            expected_cost: "0".into(),
            max_request_bytes: 4096,
            max_requests_per_day: 2,
            expires_at_ms: 10_000,
            revocation: "local tokens erased; provider deletion not promised".into(),
        }
    }
    #[test]
    fn default_offline_then_consent_then_revoke() {
        let m = manifest();
        let mut p = EgressPolicy::offline();
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://stub.example.com/x",
                10,
                1
            )
            .is_err());
        p.consent(ConnectorConsent::grant(&m, 1).unwrap());
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://stub.example.com/x",
                10,
                2
            )
            .is_ok());
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://other.example.com/x",
                10,
                2
            )
            .is_err());
        assert!(p
            .authorize(std::slice::from_ref(&m), "http://stub.example.com/x", 10, 2)
            .is_err());
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://stub.example.com/x",
                5000,
                2
            )
            .is_err());
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://stub.example.com/x",
                10,
                3
            )
            .is_ok());
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://stub.example.com/x",
                10,
                4
            )
            .unwrap_err()
            .contains("daily"));
        assert!(p.revoke("test-stub", 5));
        assert!(p
            .authorize(
                std::slice::from_ref(&m),
                "https://stub.example.com/x",
                10,
                86_400_006
            )
            .is_err());
        assert!(p.is_offline(&[m], 6));
        let mut changed = manifest();
        changed.endpoints.push("extra.example.com".into());
        let mut p2 = EgressPolicy::offline();
        p2.consent(ConnectorConsent::grant(&manifest(), 1).unwrap());
        assert!(
            p2.authorize(&[changed], "https://extra.example.com/", 1, 2)
                .is_err(),
            "consent is bound to exact manifest digest"
        );
    }
}
