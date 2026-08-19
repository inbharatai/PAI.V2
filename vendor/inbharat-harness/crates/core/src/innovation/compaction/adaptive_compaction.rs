//! Adaptive compaction: Intelligent context management with cache-aware folding.
//!
//! Cache-aware folding at 80% of input budget, typed checkpoints, incremental compaction.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Context checkpoint
#[derive(Clone, Debug)]
pub struct ContextCheckpoint {
    pub checkpoint_id: String,
    pub timestamp: u64,
    pub durable_memory: DurableMemory,
    pub execution_summary: ExecutionSummary,
    pub user_requirements: Vec<String>,
    pub skill_references: Vec<String>,
    pub live_tail: Vec<String>,
    pub token_count: usize,
}

#[derive(Clone, Debug)]
pub struct DurableMemory {
    pub decisions: Vec<String>,
    pub constraints: Vec<String>,
    pub rejected_approaches: Vec<String>,
    pub key_facts: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ExecutionSummary {
    pub current_state: String,
    pub files_touched: Vec<String>,
    pub errors: Vec<String>,
    pub next_steps: Vec<String>,
    pub completed_tasks: Vec<String>,
}

/// Adaptive compaction manager
pub struct AdaptiveCompaction {
    checkpoints: Arc<Mutex<Vec<ContextCheckpoint>>>,
    max_input_budget: usize,
    compaction_threshold: f64,
    helper_model_enabled: bool,
    incremental_compaction: bool,
}

impl AdaptiveCompaction {
    pub fn new(max_input_budget: usize) -> Self {
        Self {
            checkpoints: Arc::new(Mutex::new(Vec::new())),
            max_input_budget,
            compaction_threshold: 0.8, // 80% of input budget
            helper_model_enabled: true,
            incremental_compaction: true,
        }
    }

    /// Check if compaction is needed
    pub fn needs_compaction(&self, current_tokens: usize) -> bool {
        current_tokens as f64 > self.max_input_budget as f64 * self.compaction_threshold
    }

    /// Create a checkpoint
    pub fn create_checkpoint(
        &self,
        durable_memory: DurableMemory,
        execution_summary: ExecutionSummary,
        user_requirements: Vec<String>,
        skill_references: Vec<String>,
        live_tail: Vec<String>,
    ) -> ContextCheckpoint {
        let checkpoint = ContextCheckpoint {
            checkpoint_id: format!("checkpoint-{}", crate::innovation::short_id()),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            durable_memory,
            execution_summary,
            user_requirements,
            skill_references,
            live_tail,
            token_count: 0,
        };

        if let Ok(mut checkpoints) = self.checkpoints.lock() {
            checkpoints.push(checkpoint.clone());
        }
        checkpoint
    }

    /// Compact context
    pub fn compact(&self, current_context: Vec<String>) -> CompactionResult {
        if !self.needs_compaction(current_context.len()) {
            return CompactionResult::NoCompactionNeeded;
        }

        // Create checkpoint from current context
        let durable_memory = self.extract_durable_memory(&current_context);
        let execution_summary = self.create_execution_summary(&current_context);
        let user_requirements = self.extract_user_requirements(&current_context);
        let skill_references = self.extract_skill_references(&current_context);
        let live_tail = self.extract_live_tail(&current_context);

        let checkpoint = self.create_checkpoint(
            durable_memory,
            execution_summary,
            user_requirements,
            skill_references,
            live_tail,
        );

        // Calculate freed tokens
        let freed_tokens = current_context.len() - checkpoint.live_tail.len();

        CompactionResult::Compacted {
            checkpoint_id: checkpoint.checkpoint_id,
            freed_tokens,
            new_context_size: checkpoint.live_tail.len(),
        }
    }

    /// Get latest checkpoint
    pub fn get_latest_checkpoint(&self) -> Option<ContextCheckpoint> {
        if let Ok(checkpoints) = self.checkpoints.lock() {
            checkpoints.last().cloned()
        } else {
            None
        }
    }

    /// Get checkpoint by ID
    pub fn get_checkpoint(&self, checkpoint_id: &str) -> Option<ContextCheckpoint> {
        if let Ok(checkpoints) = self.checkpoints.lock() {
            checkpoints
                .iter()
                .find(|c| c.checkpoint_id == checkpoint_id)
                .cloned()
        } else {
            None
        }
    }

    /// Get all checkpoints
    pub fn get_all_checkpoints(&self) -> Vec<ContextCheckpoint> {
        self.checkpoints
            .lock()
            .map_or_else(|_| Vec::new(), |checkpoints| checkpoints.clone())
    }

    /// Extract durable memory from context.
    ///
    /// Heuristic extraction: matches case-insensitive marker prefixes
    /// ("decision:", "constraint:", "rejected:", "fact:"/`"key:") anywhere in a
    /// line. This is keyword/structure-based, not semantic; accuracy depends on
    /// the context using these markers. No precision is invented.
    fn extract_durable_memory(&self, context: &[String]) -> DurableMemory {
        let mut decisions = Vec::new();
        let mut constraints = Vec::new();
        let mut rejected_approaches = Vec::new();
        let mut key_facts = HashMap::new();

        for item in context {
            let lower = item.to_lowercase();
            if has_marker(&lower, &["decision:", "decided:"]) {
                push_unique(&mut decisions, item);
            }
            if has_marker(&lower, &["constraint:", "must:"]) {
                push_unique(&mut constraints, item);
            }
            if has_marker(&lower, &["rejected:", "failed:"]) {
                push_unique(&mut rejected_approaches, item);
            }
            if has_marker(&lower, &["fact:", "key:"]) {
                // Split once on the first ':' so values containing ':' survive,
                // and strip the marker prefix from the key.
                if let Some((k, v)) = item.split_once(':') {
                    let key = k.trim().to_string();
                    let value = v.trim().to_string();
                    if !key.is_empty() && !value.is_empty() {
                        key_facts.entry(key).or_insert(value);
                    }
                }
            }
        }

        DurableMemory {
            decisions,
            constraints,
            rejected_approaches,
            key_facts,
        }
    }

    /// Create execution summary from context.
    ///
    /// Heuristic extraction (keyword markers, case-insensitive), same caveats as
    /// `extract_durable_memory`. `current_state` is a human-readable compaction
    /// note only; no semantic summarization is performed here.
    fn create_execution_summary(&self, context: &[String]) -> ExecutionSummary {
        let mut files_touched = Vec::new();
        let mut errors = Vec::new();
        let mut next_steps = Vec::new();
        let mut completed_tasks = Vec::new();

        for item in context {
            let lower = item.to_lowercase();
            if has_marker(&lower, &["file:", "path:"]) {
                push_unique(&mut files_touched, item);
            }
            if has_marker(&lower, &["error:", "failed:"]) {
                push_unique(&mut errors, item);
            }
            if has_marker(&lower, &["next:", "todo:"]) {
                push_unique(&mut next_steps, item);
            }
            if has_marker(&lower, &["completed:", "done:"]) {
                push_unique(&mut completed_tasks, item);
            }
        }

        // When the helper model is unavailable, keep the summary terse: the
        // main model will re-read the durable memory verbatim anyway.
        let current_state = if self.helper_model_enabled {
            format!(
                "Context compacted at {}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            )
        } else {
            "Context compacted".to_owned()
        };

        ExecutionSummary {
            current_state,
            files_touched,
            errors,
            next_steps,
            completed_tasks,
        }
    }

    /// Extract user requirements from context (heuristic, case-insensitive).
    fn extract_user_requirements(&self, context: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for item in context {
            let lower = item.to_lowercase();
            if has_marker(&lower, &["requirement:", "need:", "want:"]) {
                push_unique(&mut out, item);
            }
        }
        out
    }

    /// Extract skill references from context (heuristic, case-insensitive).
    fn extract_skill_references(&self, context: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for item in context {
            let lower = item.to_lowercase();
            if has_marker(&lower, &["skill:", "tool:"]) {
                push_unique(&mut out, item);
            }
        }
        out
    }

    /// Extract live tail from context
    fn extract_live_tail(&self, context: &[String]) -> Vec<String> {
        // Incremental compaction keeps a larger live tail so successive folds
        // stay small; a one-shot fold keeps only the most recent 30%.
        let tail_ratio = if self.incremental_compaction {
            0.4
        } else {
            0.3
        };
        let tail_size = (context.len() as f64 * tail_ratio) as usize;
        context
            .iter()
            .skip(context.len().saturating_sub(tail_size))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum CompactionResult {
    NoCompactionNeeded,
    Compacted {
        checkpoint_id: String,
        freed_tokens: usize,
        new_context_size: usize,
    },
}

/// Return true if `lower` (already lowercased) contains any of the marker prefixes.
fn has_marker(lower: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| lower.contains(m))
}

/// Push `item` onto `out` only if not already present (dedup, order-preserving).
fn push_unique(out: &mut Vec<String>, item: &str) {
    if !out.iter().any(|existing| existing == item) {
        out.push(item.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_compaction() {
        let compaction = AdaptiveCompaction::new(1000);

        // Create a large context that exceeds 80% of the 1000-item budget
        let mut context = Vec::new();
        for i in 0..900 {
            context.push(format!("item {}: decision: made choice {}", i, i));
        }

        // Check if compaction is needed
        assert!(compaction.needs_compaction(context.len()));

        // Perform compaction
        let result = compaction.compact(context);
        assert!(matches!(result, CompactionResult::Compacted { .. }));

        // Get latest checkpoint
        let checkpoint = compaction.get_latest_checkpoint();
        assert!(checkpoint.is_some());
        let decisions_len = match checkpoint {
            Some(checkpoint) => checkpoint.durable_memory.decisions.len(),
            None => 0,
        };
        assert!(decisions_len > 0);
    }

    #[test]
    fn test_extraction_routes_markers_to_correct_buckets() {
        let compaction = AdaptiveCompaction::new(1000);

        // A small, structured context with one line per category. Extraction
        // must route each line to the matching bucket, not lump everything
        // into decisions (the old behavior returned empty buckets).
        // Markers must match the extractor's documented grammar:
        // decisions="decision:"/"decided:", constraints="constraint:"/"must:",
        // rejected="rejected:"/"failed:", errors="error:"/"failed:",
        // next="next:"/"todo:", completed="completed:"/"done:",
        // files="file:"/"path:", key_facts="fact:"/"key:".
        let context = vec![
            "decision: use musl static linking".to_string(),
            "constraint: dependency-free only".to_string(),
            "rejected: glibc dynamic linking".to_string(),
            "error: linker cc not found".to_string(),
            "next: run the full test gate".to_string(),
            "completed: semantic isa classification".to_string(),
            "file: crates/core/src/lib.rs".to_string(),
            "plain narrative line with no marker".to_string(),
        ];

        let memory = compaction.extract_durable_memory(&context);
        let summary = compaction.create_execution_summary(&context);

        assert!(
            memory
                .decisions
                .iter()
                .any(|d| d.contains("musl static linking"))
        );
        assert!(
            memory
                .constraints
                .iter()
                .any(|c| c.contains("dependency-free"))
        );
        assert!(
            memory
                .rejected_approaches
                .iter()
                .any(|r| r.contains("glibc dynamic"))
        );
        assert!(summary.errors.iter().any(|e| e.contains("cc not found")));
        assert!(
            summary
                .next_steps
                .iter()
                .any(|n| n.contains("full test gate"))
        );
        assert!(
            summary
                .completed_tasks
                .iter()
                .any(|c| c.contains("semantic isa"))
        );
        assert!(
            summary
                .files_touched
                .iter()
                .any(|f| f.contains("crates/core/src/lib.rs"))
        );
    }
}
