// UnoOne Power — Desktop ReAct Agent Loop
// D2: Implements the agentic reasoning loop on the backend.
// D3: Tool implementations now read from the live vault state instead of stubs.
// Model → parse tool calls → safety guard → execute tools → observe → loop.

use crate::documents;
use crate::llama::{
    Content, ConversationTurn, InferenceRequest, ModelManagerState, ToolDefinition,
};
use crate::safety::{SafetyGuardState, ToolAction};
use crate::security;
use serde::{Deserialize, Serialize};

const MAX_AGENT_STEPS: u32 = 5;

/// A single step in the agent's reasoning process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentStep {
    Thinking {
        text: String,
    },
    ToolCall {
        tool: String,
        args: serde_json::Value,
        /// `Some` only when the model genuinely reported a score.
        confidence: Option<f32>,
    },
    ToolResult {
        tool: String,
        result: String,
        approved: bool,
    },
    InvalidToolCall {
        tool: String,
        reason: String,
    },
    SafetyBlock {
        tool: String,
        reason: String,
    },
    FinalResponse {
        text: String,
    },
}

/// The result of running the agentic loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub steps: Vec<AgentStep>,
    pub final_text: String,
    pub iterations: u32,
}

/// D3: Tool implementations that use real vault state.
/// Each tool reads from the live DesktopVaultState or document processor.
struct ToolExecutor<'a> {
    vault_root: &'a str,
    vault_state: Option<&'a tauri::State<'a, crate::DesktopVaultState>>,
}

impl<'a> ToolExecutor<'a> {
    fn new(
        vault_root: &'a str,
        vault_state: Option<&'a tauri::State<'a, crate::DesktopVaultState>>,
    ) -> Self {
        Self {
            vault_root,
            vault_state,
        }
    }

    fn vault_handle(&self) -> Option<std::sync::MutexGuard<'_, Option<unoone_vault_core::Vault>>> {
        self.vault_state.and_then(|state| state.vault.lock().ok())
    }

    fn with_vault<R>(&self, f: impl FnOnce(&unoone_vault_core::Vault) -> R) -> Option<R> {
        let guard = self.vault_handle()?;
        guard.as_ref().map(f)
    }

    /// Search notes and documents in the vault
    fn search_notes(&self, query: &str, limit: Option<u64>) -> String {
        let search_query = documents::MemorySearchQuery {
            query: query.to_string(),
            memory_types: vec![
                "note".to_string(),
                "document".to_string(),
                "memory".to_string(),
            ],
            limit: limit.unwrap_or(10) as u32,
            min_relevance: 0.1,
        };

        let processor = documents::DocumentProcessor::new(self.vault_root);
        let mut results = processor.search_memories(&search_query);
        let extra = self.with_vault(|vault| {
            documents::search_migrated_contents(&search_query, self.vault_root, Some(vault))
        });
        if let Some(extra) = extra {
            results.extend(extra);
        }

        if results.is_empty() {
            format!("No results for '{}'.", query)
        } else {
            let mut output = format!("Found {} result(s):\n", results.len());
            for result in &results {
                output.push_str(&format!(
                    "- {} [{}] {:.0}% relevant\n  {}\n",
                    result.title,
                    result.memory_type,
                    result.relevance * 100.0,
                    result.preview
                ));
            }
            output
        }
    }

    /// List all documents in the vault
    fn list_documents(&self) -> String {
        let processor = documents::DocumentProcessor::new(self.vault_root);
        let mut docs = processor.list_documents();
        let seen: std::collections::HashSet<String> = docs.iter().map(|d| d.id.clone()).collect();
        let extra = self
            .with_vault(|vault| documents::list_migrated_documents(self.vault_root, Some(vault)));
        if let Some(mut extra) = extra {
            extra.retain(|d| !seen.contains(&d.id));
            docs.append(&mut extra);
        }

        if docs.is_empty() {
            "No documents in the vault.".to_string()
        } else {
            let mut output = format!("{} document(s):\n", docs.len());
            for doc in &docs {
                let type_tag = match doc.document_type {
                    documents::DocumentType::Txt => "TXT",
                    documents::DocumentType::Markdown => "MD",
                    documents::DocumentType::Pdf => "PDF",
                    documents::DocumentType::Docx => "DOCX",
                    documents::DocumentType::Csv => "CSV",
                    documents::DocumentType::Xlsx => "XLSX",
                    documents::DocumentType::Pptx => "PPTX",
                    documents::DocumentType::Image => "IMG",
                    documents::DocumentType::Audio => "AUDIO",
                    documents::DocumentType::WebPage => "WEB",
                };
                output.push_str(&format!(
                    "- {} [{}] {} bytes\n",
                    doc.title, type_tag, doc.file_size_bytes
                ));
                if let Some(wc) = doc.word_count {
                    output.push_str(&format!("  {} words\n", wc));
                }
            }
            output
        }
    }

    /// Read a specific document from the vault
    fn read_document(&self, document_id: &str) -> String {
        // After Wave 3 migration, originals live in encrypted records. The
        // decrypted path is tried first when plaintext lookup cannot succeed;
        // never silently claim plaintext content for an encrypted record.
        let plaintext_exists = std::path::PathBuf::from(self.vault_root)
            .join("VAULT")
            .join("documents")
            .read_dir()
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.path()
                        .file_stem()
                        .map(|s| s.to_string_lossy() == document_id)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if !plaintext_exists {
            let migrated = self
                .with_vault(|vault| {
                    documents::read_migrated_document_content(self.vault_root, document_id, vault)
                })
                .flatten();
            if let Some(bytes) = migrated {
                if let Ok(text) = String::from_utf8(bytes.clone()) {
                    if text.len() > 4000 {
                        return format!(
                            "{}...

[Truncated — {} total chars]",
                            &text[..4000],
                            text.len()
                        );
                    }
                    return text;
                }
                return format!(
                    "Cannot display '{}' — migrated binary document ({} bytes); extraction requires its decodable format.",
                    document_id,
                    bytes.len()
                );
            }
        }
        let result = {
            let processor = documents::DocumentProcessor::new(self.vault_root);
            processor.process_document(document_id)
        };

        if result.success {
            let text = result.extracted_text.unwrap_or_default();
            // Truncate very long documents for the agent context window
            unoone_text::truncate_bytes_with_notice(&text, 4000)
        } else {
            let error = result.error.unwrap_or_default();
            if error.contains("not yet supported") {
                format!(
                    "Cannot read '{}' — binary format. Only .txt and .md are supported.",
                    document_id
                )
            } else {
                format!("Cannot read '{}': {}", document_id, error)
            }
        }
    }

    /// Verify vault manifest integrity
    fn verify_vault(&self) -> String {
        match security::verify_manifest(self.vault_root.to_string()) {
            Ok(result) => {
                if result.manifest_valid && result.hmac_valid {
                    format!(
                        "Vault OK — {} files verified, HMAC valid.",
                        result.entries_verified
                    )
                } else {
                    let mut output = format!(
                        "Vault check failed: {}/{} files failed.",
                        result.entries_failed, result.total_entries
                    );
                    if !result.hmac_valid {
                        output.push_str(" HMAC signature INVALID.");
                    }
                    for err in &result.errors {
                        output.push_str(&format!("\n  - {}", err));
                    }
                    output
                }
            }
            Err(e) => format!("Cannot verify vault: {}", e),
        }
    }
}

/// D2: The agent loop state, held as Tauri managed state.
/// Contains the safety guard for tool review and the max steps limit.
pub struct AgentLoopState {
    pub max_steps: u32,
}

impl AgentLoopState {
    pub fn new() -> Self {
        Self {
            max_steps: MAX_AGENT_STEPS,
        }
    }
}

/// System prompt for the agentic loop.
/// Clean and direct — like Gemini/ChatGPT: identity first, tool rules second.
fn get_system_prompt() -> String {
    "You are UnoOne, a private AI assistant. You run entirely on the user's encrypted USB vault — no data leaves the device.\n\
     \n\
     Tools: search_notes, list_documents, read_document, verify_vault.\n\
     - Use tools when you need information from the vault to answer a question.\n\
     - Answer directly from your knowledge when tools aren't needed.\n\
     - If a tool call is blocked, explain briefly and try an alternative.\n\
     - Never reveal internal tool mechanics to the user — respond naturally."
        .to_string()
}

/// D3: Tool definitions available to the model.
/// Concise descriptions — models work best with brief, clear tool specs.
fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "search_notes".to_string(),
            description: "Search notes and documents in the vault by keyword.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keywords" },
                    "limit": { "type": "integer", "description": "Max results (default 10)" }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "list_documents".to_string(),
            description: "List all documents stored in the vault.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "read_document".to_string(),
            description: "Read a document's contents from the vault.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "document_id": { "type": "string", "description": "Document filename or ID" }
                },
                "required": ["document_id"]
            }),
        },
        ToolDefinition {
            name: "verify_vault".to_string(),
            description: "Verify vault integrity — checks file hashes and HMAC signatures."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

/// D3: Execute a single tool call using real vault state.
/// Returns the tool result as a string.
async fn execute_tool(
    tool_name: &str,
    args: &serde_json::Value,
    vault_root: &str,
    vault_state: &tauri::State<'_, crate::DesktopVaultState>,
) -> String {
    let executor = ToolExecutor::new(vault_root, Some(vault_state));

    match tool_name {
        "search_notes" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64());
            executor.search_notes(query, limit)
        }
        "list_documents" => executor.list_documents(),
        "read_document" => {
            let doc_id = args
                .get("document_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            executor.read_document(doc_id)
        }
        "verify_vault" => executor.verify_vault(),
        _ => format!("Unknown tool: {}", tool_name),
    }
}

/// Schema validation for tool calls. A call that names no known tool, or is
/// missing a required argument, is rejected *before* execution — the model
/// must never learn that a malformed call "worked".
pub fn validate_tool_call(tool_name: &str, args: &serde_json::Value) -> Result<(), String> {
    fn require_string_arg(args: &serde_json::Value, key: &str) -> Result<(), String> {
        match args.get(key).and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => Ok(()),
            _ => Err(format!("Missing or empty required argument '{}'", key)),
        }
    }
    match tool_name {
        "search_notes" => require_string_arg(args, "query"),
        "list_documents" => Ok(()),
        "read_document" => require_string_arg(args, "document_id"),
        "verify_vault" => Ok(()),
        other => Err(format!("Unknown tool: {}", other)),
    }
}

/// Canonical fingerprint of a call, used by the repetition circuit breaker.
pub fn tool_call_fingerprint(tool_name: &str, args: &serde_json::Value) -> String {
    format!(
        "{}:{}",
        tool_name,
        serde_json::to_string(args).unwrap_or_default()
    )
}

/// True when `recent` ends with `incoming` repeated `threshold` consecutive
/// times — the classic model loop where it calls the same tool with identical
/// arguments forever.
pub fn is_repetitive_loop(recent: &[String], incoming: &str, threshold: usize) -> bool {
    if threshold == 0 || recent.len() < threshold {
        return false;
    }
    recent.iter().rev().take(threshold).all(|f| f == incoming)
}

/// D2/D3: Run the full ReAct agent loop for a user message.
///
/// The loop:
/// 1. Send user message + system prompt + tools to the model via llama-server
/// 2. If model responds with tool calls: parse each → safety review → execute → observe → loop
/// 3. If model responds with plain text (no tool calls): return as final answer
/// 4. Maximum iterations: MAX_AGENT_STEPS
#[tauri::command]
pub async fn agent_chat(
    message: String,
    conversation_history: Vec<ConversationTurn>,
    model_state: tauri::State<'_, ModelManagerState>,
    safety_state: tauri::State<'_, SafetyGuardState>,
    agent_state: tauri::State<'_, AgentLoopState>,
    vault_state: tauri::State<'_, crate::DesktopVaultState>,
) -> Result<AgentResult, String> {
    let mut steps = Vec::new();
    let mut history = conversation_history;

    // D3: Get vault root from state for real tool execution
    let vault_root = {
        let root = vault_state
            .vault_root
            .lock()
            .map_err(|e| format!("State lock error: {}", e))?;
        root.clone()
    };

    if vault_root.is_empty() {
        return Err("No vault is connected. Please connect a vault first.".to_string());
    }

    // Add the user message to history
    history.push(ConversationTurn {
        role: "user".to_string(),
        content: Content::text(message),
        tool_calls: None,
        tool_call_id: None,
    });

    let tool_definitions = get_tool_definitions();
    let max_steps = agent_state.max_steps;
    // Whole-loop deadline so a model cannot hang the agent indefinitely.
    let loop_deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    // Circuit breaker over repeated identical (tool, args) calls.
    const REPETITION_THRESHOLD: usize = 2;
    let mut recent_fingerprints: Vec<String> = Vec::new();

    for _iteration in 1..=max_steps {
        if std::time::Instant::now() > loop_deadline {
            steps.push(AgentStep::FinalResponse {
                text: "Agent loop hit its 240 s deadline before the model produced a final answer. The partial steps above are everything that actually happened.".to_string(),
            });
            break;
        }
        // 1. Call the model
        let request = InferenceRequest {
            prompt: String::new(), // Using conversation history instead
            system_prompt: Some(get_system_prompt()),
            conversation_history: history.clone(),
            max_tokens: Some(4096),
            temperature: Some(0.7),
            stop_sequences: None,
            tools: Some(tool_definitions.clone()),
        };

        let port = *model_state
            .server_port
            .lock()
            .map_err(|e| format!("State lock error: {}", e))?;
        let response = {
            let manager = model_state.manager.lock().await;
            manager
                .as_ref()
                .ok_or("Model manager not initialized")?
                .send_completion(&request, port)
                .await?
        };

        // 2. Check if model wants to call tools
        if let Some(tool_calls) = &response.tool_calls {
            if !tool_calls.is_empty() {
                // Process each tool call through safety then execute
                for tc in tool_calls {
                    // 2a0. Schema validation BEFORE anything runs. A malformed
                    // or unknown call is rejected and the model is told why —
                    // it must never learn a malformed call "worked".
                    if let Err(reason) = validate_tool_call(&tc.name, &tc.arguments) {
                        steps.push(AgentStep::InvalidToolCall {
                            tool: tc.name.clone(),
                            reason: reason.clone(),
                        });
                        history.push(ConversationTurn {
                            role: "tool".to_string(),
                            content: Content::text(format!(
                                "Tool call rejected before execution: {}",
                                reason
                            )),
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                        });
                        continue;
                    }

                    // 2a1. Repetition circuit breaker: identical call again?
                    let fingerprint = tool_call_fingerprint(&tc.name, &tc.arguments);
                    if is_repetitive_loop(&recent_fingerprints, &fingerprint, REPETITION_THRESHOLD)
                    {
                        let reason = format!(
                            "Identical call repeated {} times; refusing to loop. Synthesize an answer from existing results.",
                            REPETITION_THRESHOLD
                        );
                        steps.push(AgentStep::InvalidToolCall {
                            tool: tc.name.clone(),
                            reason: reason.clone(),
                        });
                        history.push(ConversationTurn {
                            role: "tool".to_string(),
                            content: Content::text(reason),
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                        });
                        continue;
                    }
                    recent_fingerprints.push(fingerprint);

                    // 2a2. Confidence is Some only when genuinely reported by
                    // the model. Unmeasured is unmeasured — never 1.0.
                    let confidence = tc
                        .arguments
                        .get("confidence")
                        .and_then(|v| v.as_f64())
                        .map(|f| f as f32);
                    let action = ToolAction {
                        action_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        parameters: tc.arguments.clone(),
                        confidence,
                        raw_output: String::new(),
                    };

                    // 2b. Safety review
                    let verdict = {
                        let mut guard = safety_state
                            .guard
                            .lock()
                            .map_err(|e| format!("State lock error: {}", e))?;
                        guard.review_action(&action)
                    };

                    if !verdict.approved {
                        steps.push(AgentStep::SafetyBlock {
                            tool: tc.name.clone(),
                            reason: verdict.reason.clone(),
                        });
                        // Feed the blocked result back as a tool observation
                        history.push(ConversationTurn {
                            role: "tool".to_string(),
                            content: Content::text(format!(
                                "Tool '{}' was blocked: {}",
                                tc.name, verdict.reason
                            )),
                            tool_calls: None,
                            tool_call_id: Some(tc.id.clone()),
                        });
                        continue;
                    }

                    steps.push(AgentStep::ToolCall {
                        tool: tc.name.clone(),
                        args: tc.arguments.clone(),
                        confidence: action.confidence,
                    });

                    // 2c. Execute the approved tool using real vault state
                    let args = verdict
                        .modified_parameters
                        .as_ref()
                        .unwrap_or(&tc.arguments);
                    let result = execute_tool(&tc.name, args, &vault_root, &vault_state).await;

                    steps.push(AgentStep::ToolResult {
                        tool: tc.name.clone(),
                        result: result.clone(),
                        approved: true,
                    });

                    // 2d. Feed result back to model
                    history.push(ConversationTurn {
                        role: "tool".to_string(),
                        content: Content::text(result),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                    });
                }

                // Continue the loop — the model will see the tool results
                continue;
            }
        }

        // 3. No tool calls — model gave a final answer
        steps.push(AgentStep::FinalResponse {
            text: response.text.clone(),
        });
        history.push(ConversationTurn {
            role: "assistant".to_string(),
            content: Content::text(response.text),
            tool_calls: None,
            tool_call_id: None,
        });
        break;
    }

    // If we exhausted max steps without a final response
    if !steps
        .iter()
        .any(|s| matches!(s, AgentStep::FinalResponse { .. }))
    {
        steps.push(AgentStep::FinalResponse {
            text: "I need more steps to complete this. Could you rephrase or be more specific?"
                .to_string(),
        });
    }

    let final_text = steps
        .iter()
        .rev()
        .find_map(|s| {
            if let AgentStep::FinalResponse { text } = s {
                Some(text.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let iterations = steps.len() as u32;

    Ok(AgentResult {
        final_text,
        steps,
        iterations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- schema validation: correct tool, wrong tool, malformed, missing args --

    #[test]
    fn valid_search_notes_call_accepted() {
        assert!(validate_tool_call("search_notes", &json!({"query": "meeting"})).is_ok());
    }

    #[test]
    fn valid_list_documents_call_accepted() {
        assert!(validate_tool_call("list_documents", &json!({})).is_ok());
    }

    #[test]
    fn valid_read_document_call_accepted() {
        assert!(validate_tool_call("read_document", &json!({"document_id": "abc"})).is_ok());
    }

    #[test]
    fn valid_verify_vault_call_accepted() {
        assert!(validate_tool_call("verify_vault", &json!({})).is_ok());
    }

    #[test]
    fn unknown_tool_rejected_and_named() {
        let err = validate_tool_call("delete_everything", &json!({})).unwrap_err();
        assert!(err.contains("Unknown tool"));
        assert!(err.contains("delete_everything"));
    }

    #[test]
    fn missing_query_arg_rejected() {
        let err = validate_tool_call("search_notes", &json!({})).unwrap_err();
        assert!(err.contains("query"));
    }

    #[test]
    fn empty_query_arg_rejected() {
        assert!(validate_tool_call("search_notes", &json!({"query": "   "})).is_err());
    }

    #[test]
    fn wrong_type_query_arg_rejected() {
        assert!(validate_tool_call("search_notes", &json!({"query": 42})).is_err());
    }

    #[test]
    fn missing_document_id_rejected() {
        assert!(validate_tool_call("read_document", &json!({})).is_err());
    }

    // -- repetition circuit breaker ---------------------------------------------

    #[test]
    fn fingerprint_stable_for_same_call() {
        let f1 = tool_call_fingerprint("search_notes", &json!({"query": "x"}));
        let f2 = tool_call_fingerprint("search_notes", &json!({"query": "x"}));
        assert_eq!(f1, f2);
    }

    #[test]
    fn fingerprint_differs_for_different_args() {
        let f1 = tool_call_fingerprint("search_notes", &json!({"query": "x"}));
        let f2 = tool_call_fingerprint("search_notes", &json!({"query": "y"}));
        assert_ne!(f1, f2);
    }

    #[test]
    fn repeated_identical_call_detected() {
        let f = tool_call_fingerprint("search_notes", &json!({"query": "x"}));
        let recent = vec![f.clone(), f.clone()];
        assert!(is_repetitive_loop(&recent, &f, 2));
    }

    #[test]
    fn single_repetition_not_flagged_below_threshold() {
        let f = tool_call_fingerprint("search_notes", &json!({"query": "x"}));
        let recent = vec![f.clone()];
        assert!(!is_repetitive_loop(&recent, &f, 2));
    }

    #[test]
    fn distinct_calls_not_flagged() {
        let f1 = tool_call_fingerprint("search_notes", &json!({"query": "x"}));
        let f2 = tool_call_fingerprint("list_documents", &json!({}));
        let recent = vec![f1, f2.clone()];
        assert!(!is_repetitive_loop(&recent, &f2, 2));
    }

    #[test]
    fn zero_threshold_never_flags() {
        let f = "x".to_string();
        assert!(!is_repetitive_loop(std::slice::from_ref(&f), &f, 0));
    }

    // -- confidence semantics -----------------------------------------------------

    #[test]
    fn unmeasured_confidence_is_none_not_one() {
        // Regression: the old code defaulted missing confidence to 1.0, letting
        // any unmeasured call silently clear the Standard threshold.
        let args = json!({"query": "x"});
        let confidence = args
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32);
        assert_eq!(confidence, None);
    }
}
