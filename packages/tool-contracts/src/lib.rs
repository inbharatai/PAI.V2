//! PAI tool contracts — the canonical cross-platform tool vocabulary.
//!
//! `tools.v1.json` (embedded below via `include_str!`) is the single source of truth for
//! every tool PAI exposes on any host: canonical id, JSON argument schema, platform
//! availability, permission, risk class, timeout, verification, and audit metadata.
//!
//! The companion drift gate is `scripts/check_tool_contract_sync.py`: it parses the REAL
//! Android production tables (`CanonicalToolRegistry`, `SafetyGuard`, `ToolPermissionRegistry`)
//! and fails when the contract and the Kotlin source disagree in either direction. The
//! desktop registry is pinned to this contract by a test in `apps/desktop/src-tauri`
//! (`harness_bridge` registry-vs-contract test). Follows the proven speech-contract
//! pattern (`packages/speech-contracts` + `scripts/check_speech_language_sync.py`).
//!
//! Android tool ids are PINNED — they match trained tool-call datasets and are never
//! renamed. Cross-platform concepts are linked with the `equivalent` field instead.
//!
//! Risk classes reuse the Android SafetyGuard tiers
//! (`DIRECT | CONFIRM | STRONG_CONFIRM | BLOCK`); harness-registered tools map by
//! confirmation mode (`Always -> STRONG_CONFIRM`, `OnSideEffect -> CONFIRM`,
//! `Never -> DIRECT`).

use serde::{Deserialize, Serialize};

/// The checked-in single source of truth, embedded at compile time.
pub const TOOLS_JSON: &str = include_str!("../tools.v1.json");

pub const SCHEMA_ID: &str = "inbharat.pai.tools.v1";
pub const RISK_CLASSES: [&str; 4] = ["DIRECT", "CONFIRM", "STRONG_CONFIRM", "BLOCK"];
pub const PLATFORMS: [&str; 2] = ["android", "desktop"];

/// One argument of a tool (Android-style typed params; desktop tools carry `input_schema` instead).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolParam {
    pub name: String,
    /// `"string" | "integer" | "boolean" | "number"` or `{"type":"array","items":{...}}`.
    #[serde(rename = "type")]
    pub param_type: serde_json::Value,
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

/// One tool entry of the contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    pub id: String,
    pub description: String,
    pub platforms: Vec<String>,
    pub risk_class: String,
    /// `"none"`, `"runtime:PERMISSION"`, `"accessibility"`, `"media_projection"`,
    /// or `"capability:Capability"` — joined with ` + ` when multiple apply.
    pub permission: String,
    #[serde(default)]
    pub params: Vec<ToolParam>,
    /// Full JSON argument schema string (harness-registered desktop tools).
    #[serde(default)]
    pub input_schema: Option<String>,
    /// Milliseconds; `None` = no explicit per-tool timeout (see `timeout_note`).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    pub timeout_note: String,
    pub verification: String,
    pub audit: bool,
    /// `"active"` or `"blocked"` (BLOCK-tier tools have no executor and never run).
    pub status: String,
    #[serde(default)]
    pub equivalent: Option<String>,
    #[serde(default)]
    pub legacy: bool,
    #[serde(default)]
    pub legacy_note: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Harness manifest facts for desktop tools (verified against the live registry).
    #[serde(default)]
    pub harness: Option<HarnessFacts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessFacts {
    pub side_effect: String,
    pub confirmation_mode: String,
    pub capabilities: Vec<String>,
    pub supported_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContract {
    pub schema: String,
    pub description: String,
    pub risk_class_mapping: serde_json::Value,
    pub tools: Vec<ToolEntry>,
}

impl ToolContract {
    /// Parse the embedded contract.
    pub fn parse() -> Result<Self, String> {
        let contract: ToolContract = serde_json::from_str(TOOLS_JSON)
            .map_err(|e| format!("tools.v1.json parse error: {e}"))?;
        contract.verify_invariants()?;
        Ok(contract)
    }

    pub fn tool(&self, id: &str) -> Option<&ToolEntry> {
        self.tools.iter().find(|t| t.id == id)
    }

    pub fn ids_for_platform(&self, platform: &str) -> Vec<String> {
        self.tools
            .iter()
            .filter(|t| t.platforms.iter().any(|p| p == platform) && t.status == "active")
            .map(|t| t.id.clone())
            .collect()
    }

    pub fn android_active_ids(&self) -> Vec<String> {
        self.ids_for_platform("android")
    }

    pub fn desktop_active_ids(&self) -> Vec<String> {
        self.ids_for_platform("desktop")
    }

    /// Structural invariants that must hold for the contract to be valid. Called on every
    /// parse, and re-asserted by the Python sync gate so both platforms enforce them.
    pub fn verify_invariants(&self) -> Result<(), String> {
        if self.schema != SCHEMA_ID {
            return Err(format!("schema must be {SCHEMA_ID}, got {}", self.schema));
        }
        let mut seen = std::collections::HashSet::new();
        for t in &self.tools {
            if !seen.insert(t.id.as_str()) {
                return Err(format!("duplicate tool id: {}", t.id));
            }
            if !RISK_CLASSES.contains(&t.risk_class.as_str()) {
                return Err(format!("{}: invalid risk_class {}", t.id, t.risk_class));
            }
            if t.platforms.is_empty() {
                return Err(format!("{}: no platforms", t.id));
            }
            for p in &t.platforms {
                if !PLATFORMS.contains(&p.as_str()) {
                    return Err(format!("{}: unknown platform {p}", t.id));
                }
            }
            // BLOCK-tier tools must never be executable.
            if t.risk_class == "BLOCK" && t.status != "blocked" {
                return Err(format!("{}: BLOCK risk must carry status=blocked", t.id));
            }
            if t.status == "blocked" && t.risk_class != "BLOCK" {
                return Err(format!(
                    "{}: status=blocked requires risk_class=BLOCK",
                    t.id
                ));
            }
            // Desktop tools need either typed params or a JSON input schema.
            if t.platforms.contains(&"desktop".to_string()) && t.params.is_empty() {
                let schema = t.input_schema.as_deref().ok_or_else(|| {
                    format!("{}: desktop tool needs params or input_schema", t.id)
                })?;
                serde_json::from_str::<serde_json::Value>(schema)
                    .map_err(|e| format!("{}: input_schema is not valid JSON: {e}", t.id))?;
            }
            // Equivalences must resolve to a real tool id.
            if let Some(eq) = &t.equivalent {
                if self.tool(eq).is_none() {
                    return Err(format!("{}: equivalent '{eq}' does not exist", t.id));
                }
            }
            // Permission syntax must be one of the known forms.
            for perm in t.permission.split(" + ") {
                let ok = perm == "none"
                    || perm.starts_with("runtime:")
                    || perm == "accessibility"
                    || perm == "media_projection"
                    || perm.starts_with("capability:");
                if !ok {
                    return Err(format!("{}: invalid permission '{perm}'", t.id));
                }
            }
            // CONFIRM/STRONG_CONFIRM android tools are bounded by the confirmation window.
            if t.platforms.contains(&"android".to_string())
                && (t.risk_class == "CONFIRM" || t.risk_class == "STRONG_CONFIRM")
                && t.timeout_ms != Some(60_000)
            {
                return Err(format!(
                    "{}: android CONFIRM/STRONG_CONFIRM must carry the 60s confirmation-window timeout",
                    t.id
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_parses_and_holds_invariants() {
        let c = ToolContract::parse().expect("contract must parse");
        assert_eq!(c.tools.len(), 62);
    }

    #[test]
    fn android_has_exactly_the_42_pinned_registry_tools() {
        let c = ToolContract::parse().unwrap();
        let mut ids = c.android_active_ids();
        ids.sort();
        assert_eq!(ids.len(), 42, "CanonicalToolRegistry has exactly 42 tools");
        // Spot-check pinned ids that must never be renamed.
        for pinned in [
            "create_note",
            "search_notes",
            "system_control",
            "send_whatsapp",
            "open_calendar_insert",
            "detect_objects",
            "describe_scene",
            "secure_browser_task",
            "send_prepared_whatsapp",
            "create_calendar_event",
        ] {
            assert!(
                ids.contains(&pinned.to_string()),
                "missing pinned tool {pinned}"
            );
        }
    }

    #[test]
    fn android_blocked_tier_tools_never_execute() {
        let c = ToolContract::parse().unwrap();
        let blocked: Vec<_> = c
            .tools
            .iter()
            .filter(|t| t.status == "blocked")
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(
            blocked,
            [
                "access_passwords",
                "install_app",
                "make_payment",
                "send_message",
                "silent_control"
            ],
            "the five SafetyGuard BLOCK-tier classes must be present and blocked"
        );
    }

    #[test]
    fn desktop_registry_is_fully_declared() {
        let c = ToolContract::parse().unwrap();
        let mut ids = c.desktop_active_ids();
        ids.sort();
        assert_eq!(
            ids,
            [
                "agent.spawn",
                "browser.act",
                "doc.create",
                "fs.list",
                "fs.mkdir",
                "fs.read",
                "fs.write",
                "pai.list_documents",
                "pai.read_document",
                "pai.search_notes",
                "pai.verify_vault",
                "process.run",
                "web.preview",
                "workspace.patch",
                "workspace.search",
            ],
            "15 desktop tools: 4 read-lane, 5 harness builtins, 6 bridge tools"
        );
    }

    #[test]
    fn cross_platform_equivalences_resolve() {
        let c = ToolContract::parse().unwrap();
        let pai = c.tool("pai.search_notes").unwrap();
        assert_eq!(pai.equivalent.as_deref(), Some("search_notes"));
        assert!(c.tool("search_notes").is_some());
    }

    #[test]
    fn harness_risk_mapping_is_documented_and_applied() {
        let c = ToolContract::parse().unwrap();
        for (id, expected) in [
            ("fs.read", "DIRECT"),
            ("fs.write", "CONFIRM"),
            ("process.run", "STRONG_CONFIRM"),
            ("agent.spawn", "DIRECT"),
        ] {
            let t = c.tool(id).unwrap();
            assert_eq!(t.risk_class, expected, "{id} risk mapping");
        }
    }

    #[test]
    fn every_tool_declares_verification_and_audit() {
        let c = ToolContract::parse().unwrap();
        for t in &c.tools {
            assert!(
                !t.verification.is_empty(),
                "{} must declare verification",
                t.id
            );
            assert!(t.audit, "{} must be audited", t.id);
        }
    }
}
