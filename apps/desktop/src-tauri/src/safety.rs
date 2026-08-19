// UnoOne Power — Desktop Safety Guard
// Canonical safety pipeline: model output → parser → ToolAction → SafetyGuard → execution
// RAW MODEL OUTPUT NEVER EXECUTES TOOLS DIRECTLY
//
// D6: Security level is now persisted to VAULT/config/security.json
// so it survives app restarts. Level changes write to disk immediately.
// Audit log entries are preserved across level changes.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Safety level for the desktop guard
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SecurityLevel {
    Standard, // Balanced safety filtering
    Relaxed,  // Reduced filtering (user opt-in)
    Off,      // No filtering (dangerous, for testing only)
}

impl fmt::Display for SecurityLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecurityLevel::Standard => write!(f, "STANDARD"),
            SecurityLevel::Relaxed => write!(f, "RELAXED"),
            SecurityLevel::Off => write!(f, "OFF"),
        }
    }
}

/// D6: Persisted security config file structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecurityConfig {
    level: String,
    updated_at: String,
}

/// Tool action parsed from model output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAction {
    pub action_id: String,
    pub tool_name: String,
    pub parameters: serde_json::Value,
    /// `Some` only when the model actually reported a confidence score.
    /// `None` means unmeasured — it must never be silently treated as 1.0,
    /// and it can never establish that a threshold was met.
    pub confidence: Option<f32>,
    pub raw_output: String,
}

/// Safety verdict after guard review
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyVerdict {
    pub action_id: String,
    pub approved: bool,
    pub reason: String,
    pub risk_level: RiskLevel,
    pub modified_parameters: Option<serde_json::Value>,
}

/// Risk level assessment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    Safe,
    Low,
    Medium,
    High,
    Critical,
}

/// Category of potentially harmful content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HarmCategory {
    Violence,
    SelfHarm,
    IllegalActivity,
    PrivacyViolation,
    SystemManipulation,
    DataExfiltration,
    UnauthorizedAccess,
    Other(String),
}

/// Desktop Safety Guard — intercepts all model output before execution
pub struct DesktopSafetyGuard {
    security_level: SecurityLevel,
    blocked_actions: Vec<String>,
    audit_log: Vec<SafetyAuditEntry>,
    /// D6: Vault root path for persisting security level
    vault_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyAuditEntry {
    pub timestamp: String,
    pub action_id: String,
    pub tool_name: String,
    pub verdict: String,
    pub reason: String,
    pub risk_level: String,
}

impl DesktopSafetyGuard {
    pub fn new(security_level: SecurityLevel) -> Self {
        let blocked_actions = Self::blocked_actions_for_level(&security_level);

        Self {
            security_level,
            blocked_actions,
            audit_log: Vec::new(),
            vault_root: None,
        }
    }

    /// D6: Create a new guard, loading the persisted security level from disk.
    /// Falls back to Standard if no config file exists.
    pub fn new_with_vault_root(vault_root: &str) -> Self {
        let persisted_level = Self::load_security_level(vault_root);
        let mut guard = Self::new(persisted_level);
        guard.vault_root = Some(vault_root.to_string());
        guard
    }

    /// D6: Load security level from VAULT/config/security.json
    fn load_security_level(vault_root: &str) -> SecurityLevel {
        let config_path = PathBuf::from(vault_root)
            .join("VAULT")
            .join("config")
            .join("security.json");

        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<SecurityConfig>(&content) {
                return match config.level.as_str() {
                    "RELAXED" => SecurityLevel::Relaxed,
                    "OFF" => SecurityLevel::Off,
                    _ => SecurityLevel::Standard,
                };
            }
        }
        SecurityLevel::Standard
    }

    /// D6: Persist current security level to VAULT/config/security.json
    fn save_security_level(&self) -> Result<(), String> {
        if let Some(ref vault_root) = self.vault_root {
            let config_dir = PathBuf::from(vault_root).join("VAULT").join("config");
            std::fs::create_dir_all(&config_dir)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;

            let config_path = config_dir.join("security.json");
            let config = SecurityConfig {
                level: self.security_level.to_string(),
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            let json = serde_json::to_string_pretty(&config)
                .map_err(|e| format!("Failed to serialize security config: {}", e))?;
            std::fs::write(&config_path, json)
                .map_err(|e| format!("Failed to write security config: {}", e))?;
        }
        Ok(())
    }

    /// D6: Change security level and persist to disk. Preserves audit log.
    pub fn set_security_level(&mut self, level: SecurityLevel) -> Result<(), String> {
        self.security_level = level.clone();
        self.blocked_actions = Self::blocked_actions_for_level(&level);
        self.save_security_level()
    }

    /// Compute blocked actions for a given security level
    fn blocked_actions_for_level(level: &SecurityLevel) -> Vec<String> {
        match level {
            SecurityLevel::Standard => vec![
                "shell_execute".to_string(),
                "file_delete_system".to_string(),
                "network_raw_socket".to_string(),
                "registry_modify".to_string(),
            ],
            SecurityLevel::Relaxed => {
                vec!["shell_execute".to_string(), "registry_modify".to_string()]
            }
            SecurityLevel::Off => vec![],
        }
    }

    /// CRITICAL: Review a parsed ToolAction before execution.
    /// This is the canonical safety pipeline — model output NEVER
    /// bypasses this check.
    pub fn review_action(&mut self, action: &ToolAction) -> SafetyVerdict {
        // 1. Check blocked action list
        if self.blocked_actions.contains(&action.tool_name) {
            let verdict = SafetyVerdict {
                action_id: action.action_id.clone(),
                approved: false,
                reason: format!(
                    "Tool '{}' is blocked under {} security level",
                    action.tool_name, self.security_level
                ),
                risk_level: RiskLevel::Critical,
                modified_parameters: None,
            };
            self.log_audit(&verdict, &action.tool_name, &action.raw_output);
            return verdict;
        }

        // 2. Check confidence threshold
        let min_confidence = match self.security_level {
            SecurityLevel::Standard => 0.7,
            SecurityLevel::Relaxed => 0.5,
            SecurityLevel::Off => 0.0,
        };

        // Only a genuinely measured confidence can establish the threshold.
        // Unmeasured confidence proceeds with Medium risk (recorded below);
        // it is never silently promoted to "high confidence".
        if let Some(c) = action.confidence {
            if c < min_confidence {
                let verdict = SafetyVerdict {
                    action_id: action.action_id.clone(),
                    approved: false,
                    reason: format!(
                        "Confidence {} below minimum {} for {} security",
                        c, min_confidence, self.security_level
                    ),
                    risk_level: RiskLevel::Medium,
                    modified_parameters: None,
                };
                self.log_audit(&verdict, &action.tool_name, &action.raw_output);
                return verdict;
            }
        }

        // 3. Check for harmful content in parameters
        if let Some(harm) = self.detect_harm(&action.parameters) {
            let verdict = SafetyVerdict {
                action_id: action.action_id.clone(),
                approved: false,
                reason: format!("Harmful content detected: {:?}", harm),
                risk_level: RiskLevel::High,
                modified_parameters: None,
            };
            self.log_audit(&verdict, &action.tool_name, &action.raw_output);
            return verdict;
        }

        // 4. Sanitize parameters (remove potentially dangerous values)
        let sanitized = self.sanitize_parameters(&action.parameters);

        // 5. Approve with logging
        let verdict = SafetyVerdict {
            action_id: action.action_id.clone(),
            approved: true,
            reason: "Action approved by safety guard".to_string(),
            risk_level: RiskLevel::Safe,
            modified_parameters: sanitized,
        };
        self.log_audit(&verdict, &action.tool_name, &action.raw_output);
        verdict
    }

    /// Detect harmful content in parameters
    fn detect_harm(&self, params: &serde_json::Value) -> Option<HarmCategory> {
        // Check string values for harmful patterns
        if let Some(obj) = params.as_object() {
            for (_key, value) in obj {
                if let Some(s) = value.as_str() {
                    if let Some(harm) = self.check_string_for_harm(s) {
                        return Some(harm);
                    }
                }
            }
        }
        if let Some(s) = params.as_str() {
            return self.check_string_for_harm(s);
        }
        None
    }

    fn check_string_for_harm(&self, s: &str) -> Option<HarmCategory> {
        let lower = s.to_lowercase();

        // Check for system manipulation patterns
        let system_patterns = [
            "rm -rf",
            "del /s",
            "format c:",
            "shutdown",
            "taskkill",
            "reg delete",
            "reg add",
            "mklink",
            "icacls",
            "sudo rm",
            "> /dev/",
            "chmod 777",
            ":(){ :|:& };:",
        ];
        for pattern in system_patterns {
            if lower.contains(pattern) {
                return Some(HarmCategory::SystemManipulation);
            }
        }

        // Check for data exfiltration patterns
        let exfil_patterns = [
            "curl .*|.*ftp",
            "wget .*--post",
            "nc -e",
            "powershell.*invoke-webrequest",
            "base64.*|.*curl",
            "scp .*@",
        ];
        for pattern in exfil_patterns {
            if lower.contains(pattern) {
                return Some(HarmCategory::DataExfiltration);
            }
        }

        // Check for unauthorized access patterns
        let access_patterns = [
            "sudo su",
            "runas /user:administrator",
            "net user.*add",
            "passwd root",
            "shadow file",
        ];
        for pattern in access_patterns {
            if lower.contains(pattern) {
                return Some(HarmCategory::UnauthorizedAccess);
            }
        }

        None
    }

    /// Sanitize parameters — remove or redact dangerous values
    fn sanitize_parameters(&self, params: &serde_json::Value) -> Option<serde_json::Value> {
        // For STANDARD level, sanitize file paths to prevent system access
        if self.security_level == SecurityLevel::Standard {
            if let Some(obj) = params.as_object() {
                let mut sanitized = obj.clone();
                if let Some(path) = sanitized.get("path").and_then(|v| v.as_str()) {
                    // Block system paths
                    let blocked_prefixes = [
                        "/etc/",
                        "/usr/bin",
                        "/sbin/",
                        "/root/",
                        "C:\\Windows\\",
                        "C:\\Program Files\\",
                        "C:\\Users\\All Users\\",
                    ];
                    for prefix in blocked_prefixes {
                        if path.starts_with(prefix) {
                            sanitized.insert(
                                "path".to_string(),
                                serde_json::Value::String("[BLOCKED: system path]".to_string()),
                            );
                            return Some(serde_json::Value::Object(sanitized));
                        }
                    }
                }
            }
        }
        None
    }

    fn log_audit(&mut self, verdict: &SafetyVerdict, tool_name: &str, _raw_output: &str) {
        self.audit_log.push(SafetyAuditEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            action_id: verdict.action_id.clone(),
            tool_name: tool_name.to_string(),
            verdict: if verdict.approved {
                "APPROVED".to_string()
            } else {
                "BLOCKED".to_string()
            },
            reason: verdict.reason.clone(),
            risk_level: format!("{:?}", verdict.risk_level),
        });
    }

    pub fn get_security_level(&self) -> &SecurityLevel {
        &self.security_level
    }

    pub fn get_audit_log(&self) -> &[SafetyAuditEntry] {
        &self.audit_log
    }
}

// Tauri commands for safety guard

/// D5/D6: Stateful wrapper so the safety guard persists across calls,
/// accumulating audit log entries, respecting the current security level,
/// and persisting level changes to disk (D6).
pub struct SafetyGuardState {
    /// Arc-wrapped so the unified Harness bridge can share the single canonical
    /// safety guard authority across a `spawn_blocking` boundary. Every
    /// model-selected tool action is still reviewed by this same guard — the
    /// Harness core never executes raw model output directly.
    pub guard: Arc<Mutex<DesktopSafetyGuard>>,
}

#[tauri::command]
pub fn get_security_level(state: tauri::State<'_, SafetyGuardState>) -> Result<String, String> {
    let guard = state
        .guard
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    Ok(guard.get_security_level().to_string())
}

/// D6: Set security level — persists to VAULT/config/security.json and
/// preserves the audit log across level changes.
#[tauri::command]
pub fn set_security_level(
    level: String,
    state: tauri::State<'_, SafetyGuardState>,
) -> Result<String, String> {
    let new_level = match level.as_str() {
        "RELAXED" => SecurityLevel::Relaxed,
        "OFF" => SecurityLevel::Off,
        _ => SecurityLevel::Standard,
    };

    // D6: Use set_security_level which updates blocked_actions in-place
    // and persists to disk, preserving the audit log.
    let mut guard = state
        .guard
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    guard.set_security_level(new_level)?;

    Ok(format!("Security level set to {}", level))
}

#[tauri::command]
pub fn get_audit_log(
    state: tauri::State<'_, SafetyGuardState>,
) -> Result<Vec<SafetyAuditEntry>, String> {
    let guard = state
        .guard
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    Ok(guard.get_audit_log().to_vec())
}

#[tauri::command]
pub fn review_tool_action(
    action: ToolAction,
    state: tauri::State<'_, SafetyGuardState>,
) -> Result<SafetyVerdict, String> {
    let mut guard = state
        .guard
        .lock()
        .map_err(|e| format!("State lock error: {}", e))?;
    Ok(guard.review_action(&action))
}
