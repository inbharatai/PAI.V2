//! Semantic ISA: Probabilistic messages reified into discrete instructions.
//!
//! Security Context Registry + Instruction Dependency Graph + taint propagation.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Semantic instruction types
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InstructionType {
    Read,       // Read operation
    Write,      // Write operation
    Execute,    // Execute operation
    Network,    // Network operation
    FileSystem, // File system operation
    Process,    // Process operation
    Credential, // Credential operation
    Model,      // Model operation
    Tool,       // Tool operation
    Memory,     // Memory operation
}

/// Security context for taint tracking
#[derive(Clone, Debug)]
pub struct SecurityContext {
    pub context_id: String,
    pub trust_level: TrustLevel,
    pub allowed_operations: HashSet<InstructionType>,
    pub taint_labels: HashSet<String>,
    pub data_flow_pedigree: Vec<DataFlowNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    Trusted,     // Trusted system component
    SemiTrusted, // Semi-trusted user code
    Untrusted,   // Untrusted external input
    Malicious,   // Known malicious input
}

#[derive(Clone, Debug)]
pub struct DataFlowNode {
    pub node_id: String,
    pub source: String,
    pub operation: String,
    pub taint_labels: HashSet<String>,
    pub timestamp: u64,
}

/// Semantic instruction
#[derive(Clone, Debug)]
pub struct SemanticInstruction {
    pub instruction_id: String,
    pub instruction_type: InstructionType,
    pub operation: String,
    pub operands: Vec<String>,
    pub security_context: SecurityContext,
    pub taint_labels: HashSet<String>,
    pub dependencies: Vec<String>,
    pub side_effects: Vec<String>,
}

/// Instruction dependency graph
#[derive(Clone, Debug)]
pub struct InstructionDependencyGraph {
    pub nodes: HashMap<String, SemanticInstruction>,
    pub edges: HashMap<String, Vec<String>>,
    pub execution_order: Vec<String>,
}

/// Semantic ISA engine
pub struct SemanticISA {
    security_contexts: Arc<Mutex<HashMap<String, SecurityContext>>>,
    instruction_graphs: Arc<Mutex<HashMap<String, InstructionDependencyGraph>>>,
    taint_registry: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    execution_history: Arc<Mutex<Vec<ExecutionEvent>>>,
}

#[derive(Clone, Debug)]
pub struct ExecutionEvent {
    pub event_id: String,
    pub instruction_id: String,
    pub security_context: String,
    pub taint_labels: HashSet<String>,
    pub timestamp: u64,
    pub result: ExecutionResult,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecutionResult {
    Success,
    Failure,
    Blocked,
    Cancelled,
}

impl SemanticISA {
    pub fn new() -> Self {
        Self {
            security_contexts: Arc::new(Mutex::new(HashMap::new())),
            instruction_graphs: Arc::new(Mutex::new(HashMap::new())),
            taint_registry: Arc::new(Mutex::new(HashMap::new())),
            execution_history: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a security context
    pub fn create_security_context(
        &self,
        context_id: String,
        trust_level: TrustLevel,
        allowed_operations: HashSet<InstructionType>,
    ) -> SecurityContext {
        let context = SecurityContext {
            context_id: context_id.clone(),
            trust_level,
            allowed_operations,
            taint_labels: HashSet::new(),
            data_flow_pedigree: Vec::new(),
        };

        if let Ok(mut contexts) = self.security_contexts.lock() {
            contexts.insert(context_id, context.clone());
        }
        context
    }

    /// Reify a probabilistic message into a semantic instruction
    pub fn reify_instruction(
        &self,
        message: &str,
        context_id: &str,
    ) -> Option<SemanticInstruction> {
        let instruction = self
            .security_contexts
            .lock()
            .ok()
            .and_then(|contexts| contexts.get(context_id).cloned())
            .and_then(|context| self.parse_message(message, &context))?;

        // Add to dependency graph
        self.add_to_dependency_graph(instruction.clone());

        // Propagate taint
        self.propagate_taint(&instruction);

        Some(instruction)
    }

    /// Check if an instruction is allowed in a security context
    pub fn is_allowed(&self, instruction: &SemanticInstruction, context_id: &str) -> bool {
        self.security_contexts
            .lock()
            .ok()
            .and_then(|contexts| contexts.get(context_id).cloned())
            .is_some_and(|ctx| {
                // Check if instruction type is allowed
                if !ctx
                    .allowed_operations
                    .contains(&instruction.instruction_type)
                {
                    return false;
                }

                // Check taint labels
                if !instruction.taint_labels.is_subset(&ctx.taint_labels) {
                    return false;
                }

                // Check trust level
                ctx.trust_level >= TrustLevel::SemiTrusted
            })
    }

    /// Execute an instruction with taint tracking
    pub fn execute_instruction(&self, instruction: &SemanticInstruction) -> ExecutionResult {
        // Record execution event
        let event = ExecutionEvent {
            event_id: format!("event-{}", crate::innovation::short_id()),
            instruction_id: instruction.instruction_id.clone(),
            security_context: instruction.security_context.context_id.clone(),
            taint_labels: instruction.taint_labels.clone(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            result: ExecutionResult::Success,
        };

        if let Ok(mut history) = self.execution_history.lock() {
            history.push(event);
        }

        // Check for dangerous sinks
        if self.is_dangerous_sink(instruction) {
            return ExecutionResult::Blocked;
        }

        ExecutionResult::Success
    }

    /// Get execution history
    pub fn get_execution_history(&self) -> Vec<ExecutionEvent> {
        self.execution_history
            .lock()
            .map_or_else(|_| Vec::new(), |history| history.clone())
    }

    /// Get instruction dependency graph
    pub fn get_dependency_graph(&self, graph_id: &str) -> Option<InstructionDependencyGraph> {
        self.instruction_graphs
            .lock()
            .ok()
            .and_then(|graphs| graphs.get(graph_id).cloned())
    }

    /// Propagate taint through the system
    fn propagate_taint(&self, instruction: &SemanticInstruction) {
        if let Ok(mut registry) = self.taint_registry.lock() {
            // Add taint labels to registry
            for label in &instruction.taint_labels {
                registry
                    .entry(label.clone())
                    .or_default()
                    .insert(instruction.instruction_id.clone());
            }

            // Propagate to dependencies
            for dep in &instruction.dependencies {
                if let Some(dep_labels) = registry.get(dep) {
                    for label in dep_labels.clone() {
                        registry
                            .entry(label.clone())
                            .or_default()
                            .insert(instruction.instruction_id.clone());
                    }
                }
            }
        }
    }

    /// Parse a probabilistic message into a semantic instruction
    fn parse_message(
        &self,
        message: &str,
        context: &SecurityContext,
    ) -> Option<SemanticInstruction> {
        let instruction_id = format!("inst-{}", crate::innovation::short_id());

        // Case-insensitive word-tokenized classification. Splitting on
        // non-alphanumeric characters avoids substring false positives
        // (e.g. "store" matching inside "restore").
        let lower = message.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();

        // PRECEDENCE RULE: every matching category scores its rank and the
        // highest rank wins, regardless of keyword order in the message.
        // Dangerous sinks outrank benign operations so a message touching a
        // dangerous sink can never be downgraded by an earlier innocent
        // keyword (e.g. "read config then spawn process" is Process, not
        // Read). Rank order: Credential/Process/Network/Execute (4) >
        // Write/FileSystem/Memory (3) > Tool/Model (2) > Read (1). No
        // keyword match defaults to Read (rank 1, least privilege).
        let keyword_rank = |tokens: &[&str], words: &[&str], rank: u32| {
            if tokens.iter().any(|t| words.contains(t)) {
                rank
            } else {
                0
            }
        };
        let ranked = [
            (
                InstructionType::Credential,
                keyword_rank(&tokens, &["credential", "token", "password", "secret"], 4),
            ),
            (
                InstructionType::Process,
                keyword_rank(&tokens, &["process", "spawn", "fork"], 4),
            ),
            (
                InstructionType::Network,
                keyword_rank(
                    &tokens,
                    &["network", "http", "https", "connect", "socket"],
                    4,
                ),
            ),
            (
                InstructionType::Execute,
                keyword_rank(&tokens, &["execute", "run", "exec"], 4),
            ),
            (
                InstructionType::Write,
                keyword_rank(&tokens, &["write", "set", "update", "delete"], 3),
            ),
            (
                InstructionType::FileSystem,
                keyword_rank(&tokens, &["file", "directory", "folder", "path"], 3),
            ),
            (
                InstructionType::Memory,
                keyword_rank(&tokens, &["memory", "store", "cache"], 3),
            ),
            (
                InstructionType::Tool,
                keyword_rank(&tokens, &["tool", "function"], 2),
            ),
            (
                InstructionType::Model,
                keyword_rank(&tokens, &["model", "llm"], 2),
            ),
            (
                InstructionType::Read,
                keyword_rank(&tokens, &["read", "get", "fetch", "load"], 1),
            ),
        ];
        // Deterministic resolution. On a rank tie, the variant declared
        // EARLIER in `ranked` wins (iterator order is the tie-break), so the
        // declaration order above encodes the intended precedence within a
        // rank: concrete actions (Write) beat categories (FileSystem/Memory).
        // We fold with a strict ">" so the first maximal element is kept.
        let instruction_type = ranked
            .iter()
            .fold(
                (InstructionType::Read, 0u32),
                |(best_ty, best_rank), (ty, rank)| {
                    if *rank > best_rank {
                        (ty.clone(), *rank)
                    } else {
                        (best_ty, best_rank)
                    }
                },
            )
            .0;

        // Extract a target operand: the word after the first preposition.
        // Operands are taken from the ORIGINAL message split on whitespace
        // only (not the alphanumeric classification tokens), so path-like
        // targets such as "/var/log/app.log" survive intact instead of being
        // shattered into ["var", "log", "app", "log"].
        let prepositions = ["to", "from", "on", "into", "at", "in"];
        let words: Vec<&str> = message.split_whitespace().collect();
        let operands: Vec<String> = words
            .windows(2)
            .find(|pair| prepositions.contains(&pair[0].to_lowercase().as_str()))
            .map_or_else(Vec::new, |pair| vec![pair[1].to_string()]);

        // Side effects are derived deterministically from the classified
        // instruction type so downstream checks (is_dangerous_sink,
        // dependency analysis) have real facts to evaluate. When no operand
        // could be extracted, the target is recorded as "unknown" rather
        // than inventing a fake one.
        let target = operands
            .first()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let side_effects = match instruction_type {
            InstructionType::Read => vec![format!("read:{target}")],
            InstructionType::Write => vec![format!("file_write:{target}")],
            InstructionType::Execute => vec![format!("execute:{target}")],
            InstructionType::Network => vec![format!("network_egress:{target}")],
            InstructionType::FileSystem => vec![format!("filesystem_access:{target}")],
            InstructionType::Process => vec![format!("process_spawn:{target}")],
            InstructionType::Credential => vec![format!("credential_access:{target}")],
            InstructionType::Model => vec![format!("model_invoke:{target}")],
            InstructionType::Tool => vec![format!("tool_call:{target}")],
            InstructionType::Memory => vec![format!("memory_store:{target}")],
        };

        // `dependencies` is intentionally left empty: a single message
        // carries no information about prior instructions it depends on, so
        // there is nothing honest to populate it with.
        // `taint_labels` is intentionally left empty: labels are attached by
        // the security context / taint tracker, not inferred from free text.
        Some(SemanticInstruction {
            instruction_id,
            instruction_type,
            operation: message.to_string(),
            operands,
            security_context: context.clone(),
            taint_labels: HashSet::new(),
            dependencies: vec![],
            side_effects,
        })
    }

    /// Add instruction to dependency graph
    fn add_to_dependency_graph(&self, instruction: SemanticInstruction) {
        if let Ok(mut graphs) = self.instruction_graphs.lock() {
            // Create new graph for this instruction
            let graph_id = format!("graph-{}", instruction.instruction_id);
            let mut graph = InstructionDependencyGraph {
                nodes: HashMap::new(),
                edges: HashMap::new(),
                execution_order: vec![],
            };

            graph
                .nodes
                .insert(instruction.instruction_id.clone(), instruction.clone());
            graph
                .execution_order
                .push(instruction.instruction_id.clone());

            graphs.insert(graph_id, graph);
        }
    }

    /// Check if an instruction is a dangerous sink
    fn is_dangerous_sink(&self, instruction: &SemanticInstruction) -> bool {
        // Check for dangerous operations
        matches!(
            instruction.instruction_type,
            InstructionType::Process | InstructionType::Network | InstructionType::Credential
        ) && instruction.security_context.trust_level < TrustLevel::SemiTrusted
    }
}

impl Default for SemanticISA {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_isa() {
        let isa = SemanticISA::new();

        // Create security context. "read file test.txt" classifies as
        // FileSystem (it names a file target), so the context must permit
        // FileSystem for is_allowed to pass — this reflects the more
        // accurate classifier, not a weakened check.
        let _context = isa.create_security_context(
            "test-context".to_string(),
            TrustLevel::SemiTrusted,
            HashSet::from([
                InstructionType::Read,
                InstructionType::Write,
                InstructionType::FileSystem,
            ]),
        );

        // Reify instruction
        let instruction = isa.reify_instruction("read file test.txt", "test-context");
        assert!(instruction.is_some());

        if let Some(inst) = instruction {
            // Check if allowed
            assert!(isa.is_allowed(&inst, "test-context"));

            // Execute instruction
            let result = isa.execute_instruction(&inst);
            assert_eq!(result, ExecutionResult::Success);

            // Check execution history
            let history = isa.get_execution_history();
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].instruction_id, inst.instruction_id);
        }
    }

    fn test_isa() -> SemanticISA {
        let isa = SemanticISA::new();
        let _context = isa.create_security_context(
            "ctx".to_string(),
            TrustLevel::SemiTrusted,
            HashSet::from([
                InstructionType::Read,
                InstructionType::Write,
                InstructionType::Execute,
                InstructionType::Network,
                InstructionType::FileSystem,
                InstructionType::Process,
                InstructionType::Credential,
                InstructionType::Model,
                InstructionType::Tool,
                InstructionType::Memory,
            ]),
        );
        isa
    }

    #[test]
    fn test_parse_message_precedence_dangerous_wins() {
        let isa = test_isa();

        // Passing case: an innocent read-only message stays Read.
        let read_inst = isa.reify_instruction("read the daily report", "ctx");
        assert!(read_inst.is_some());
        if let Some(inst) = read_inst {
            assert_eq!(inst.instruction_type, InstructionType::Read);
            assert!(!inst.side_effects.is_empty());
        }

        // Failing case: dangerous sink mentioned AFTER an innocent keyword
        // must not be downgraded to Read.
        let mixed = isa.reify_instruction("write the read report", "ctx");
        assert!(mixed.is_some());
        if let Some(inst) = mixed {
            assert_eq!(inst.instruction_type, InstructionType::Write);
        }

        let dangerous = isa.reify_instruction("read config then spawn process", "ctx");
        assert!(dangerous.is_some());
        if let Some(inst) = dangerous {
            assert_eq!(inst.instruction_type, InstructionType::Process);
            assert!(
                inst.side_effects
                    .iter()
                    .any(|e| e.starts_with("process_spawn:"))
            );
        }
    }

    #[test]
    fn test_parse_message_case_insensitive_and_operands() {
        let isa = test_isa();

        // Passing case: uppercase keywords classify correctly and the
        // target operand after a preposition is extracted.
        let upper = isa.reify_instruction("WRITE FILE to /var/log/app.log", "ctx");
        assert!(upper.is_some());
        if let Some(inst) = upper {
            assert_eq!(inst.instruction_type, InstructionType::Write);
            assert_eq!(inst.operands, vec!["/var/log/app.log".to_string()]);
            assert_eq!(
                inst.side_effects,
                vec!["file_write:/var/log/app.log".to_string()]
            );
        }

        // Failing case: an unparseable message (no keywords) used to
        // silently default to Read with empty fields; it now still defaults
        // to Read but carries an honest side-effect record with an explicit
        // "unknown" target rather than empty vectors.
        let opaque = isa.reify_instruction("xyzzy plugh", "ctx");
        assert!(opaque.is_some());
        if let Some(inst) = opaque {
            assert_eq!(inst.instruction_type, InstructionType::Read);
            assert!(inst.operands.is_empty());
            assert_eq!(inst.side_effects, vec!["read:unknown".to_string()]);
        }
    }
}
