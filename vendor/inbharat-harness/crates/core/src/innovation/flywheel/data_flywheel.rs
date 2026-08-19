//! Data flywheel: Execution traces train better routers and harness-native models.
//!
//! Every routing decision produces structured records that improve future decisions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Execution record for the data flywheel
#[derive(Clone, Debug)]
pub struct ExecutionRecord {
    pub record_id: String,
    pub timestamp: u64,
    pub query: String,
    pub harness_state: HarnessStateSnapshot,
    pub routing_decision: RoutingDecisionSnapshot,
    pub execution_trace: ExecutionTraceSnapshot,
    pub outcome: OutcomeSnapshot,
    pub cost: CostSnapshot,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct HarnessStateSnapshot {
    pub available_tools: Vec<String>,
    pub current_budget: BudgetSnapshot,
    pub resource_usage: ResourceUsageSnapshot,
    pub conversation_context: String,
}

#[derive(Clone, Debug)]
pub struct BudgetSnapshot {
    pub tokens_remaining: usize,
    pub time_remaining_ms: u64,
    pub memory_remaining_mb: usize,
    pub tool_calls_remaining: usize,
}

#[derive(Clone, Debug)]
pub struct ResourceUsageSnapshot {
    pub cpu_percent: f64,
    pub memory_mb: usize,
    pub active_sessions: usize,
    pub queued_tasks: usize,
}

#[derive(Clone, Debug)]
pub struct RoutingDecisionSnapshot {
    pub selected_model: String,
    pub confidence: f64,
    pub reasoning: String,
    pub fallback_models: Vec<String>,
    pub policy: String,
}

#[derive(Clone, Debug)]
pub struct ExecutionTraceSnapshot {
    pub steps: Vec<StepSnapshot>,
    pub total_time_ms: u64,
    pub total_tokens: usize,
    pub tool_calls: Vec<ToolCallSnapshot>,
    pub errors: Vec<ErrorSnapshot>,
}

#[derive(Clone, Debug)]
pub struct StepSnapshot {
    pub step_id: String,
    pub step_type: String,
    pub input: String,
    pub output: String,
    pub duration_ms: u64,
    pub tokens_used: usize,
}

#[derive(Clone, Debug)]
pub struct ToolCallSnapshot {
    pub tool_name: String,
    pub arguments: String,
    pub result: String,
    pub duration_ms: u64,
    pub success: bool,
}

#[derive(Clone, Debug)]
pub struct ErrorSnapshot {
    pub error_type: String,
    pub message: String,
    pub step_id: Option<String>,
    pub recoverable: bool,
}

#[derive(Clone, Debug)]
pub struct OutcomeSnapshot {
    pub success: bool,
    pub quality_score: f64,
    pub user_satisfaction: Option<f64>,
    pub verification_passed: bool,
    pub side_effects: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct CostSnapshot {
    pub total_cost: f64,
    pub cost_per_token: f64,
    pub cost_per_tool_call: f64,
    pub budget_utilization: f64,
}

/// Data flywheel for continuous improvement
pub struct DataFlywheel {
    records: Arc<Mutex<Vec<ExecutionRecord>>>,
    model_performance: Arc<Mutex<HashMap<String, ModelPerformance>>>,
    routing_patterns: Arc<Mutex<HashMap<String, RoutingPattern>>>,
    improvement_suggestions: Arc<Mutex<Vec<ImprovementSuggestion>>>,
    max_records: usize,
    learning_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct ModelPerformance {
    pub model_name: String,
    pub total_uses: usize,
    pub success_rate: f64,
    pub avg_cost: f64,
    pub avg_latency_ms: u64,
    pub avg_quality: f64,
    pub task_types: HashMap<String, TaskPerformance>,
}

#[derive(Clone, Debug)]
pub struct TaskPerformance {
    pub task_type: String,
    pub uses: usize,
    pub success_rate: f64,
    pub avg_cost: f64,
    pub avg_latency_ms: u64,
}

#[derive(Clone, Debug)]
pub struct RoutingPattern {
    pub pattern_id: String,
    pub query_features: Vec<String>,
    pub best_model: String,
    pub confidence: f64,
    pub support_count: usize,
    pub last_updated: u64,
    /// Per-model running mean of realized outcome quality within this pattern.
    /// Used to decide whether a challenger model should displace `best_model`.
    pub model_quality: HashMap<String, ModelQuality>,
}

/// Running mean of realized outcome quality for one model within a routing pattern.
#[derive(Clone, Debug)]
pub struct ModelQuality {
    pub uses: usize,
    pub mean_quality: f64,
}

#[derive(Clone, Debug)]
pub struct ImprovementSuggestion {
    pub suggestion_id: String,
    pub category: String,
    pub description: String,
    pub expected_improvement: f64,
    pub confidence: f64,
    pub evidence: Vec<String>,
}

impl DataFlywheel {
    pub fn new(max_records: usize) -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
            model_performance: Arc::new(Mutex::new(HashMap::new())),
            routing_patterns: Arc::new(Mutex::new(HashMap::new())),
            improvement_suggestions: Arc::new(Mutex::new(Vec::new())),
            max_records,
            learning_enabled: true,
        }
    }

    /// Record an execution for the flywheel
    pub fn record_execution(&self, record: ExecutionRecord) {
        if !self.learning_enabled {
            return;
        }

        // Add to records
        if let Ok(mut records) = self.records.lock() {
            records.push(record.clone());

            // Limit size
            if records.len() > self.max_records {
                records.remove(0);
            }
        }

        // Update model performance
        self.update_model_performance(&record);

        // Update routing patterns
        self.update_routing_patterns(&record);

        // Generate improvement suggestions
        self.generate_improvements(&record);
    }

    /// Get performance metrics for a model
    pub fn get_model_performance(&self, model_name: &str) -> Option<ModelPerformance> {
        self.model_performance
            .lock()
            .ok()
            .and_then(|performance| performance.get(model_name).cloned())
    }

    /// Get routing pattern for a query
    ///
    /// Every recorded pattern whose `query_features` are a subset of the query is a
    /// candidate. Among candidates we pick the one with the strongest realized
    /// signal: `confidence * support_count` (confidence is the running mean of
    /// realized outcome quality for this pattern — see `update_routing_patterns`),
    /// breaking ties toward the most recently reinforced pattern. This replaces the
    /// old first-match-in-HashMap-iteration-order behaviour, which ignored all of
    /// the signal the pattern carries and was non-deterministic.
    pub fn get_routing_pattern(&self, query_features: &[String]) -> Option<RoutingPattern> {
        let patterns = self.routing_patterns.lock().ok()?;

        patterns
            .values()
            .filter(|pattern| {
                pattern
                    .query_features
                    .iter()
                    .all(|f| query_features.contains(f))
            })
            .max_by(|a, b| {
                let strength_a = a.confidence * a.support_count as f64;
                let strength_b = b.confidence * b.support_count as f64;
                strength_b
                    .partial_cmp(&strength_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // Tie-break: more recently updated pattern wins.
                    .then(a.last_updated.cmp(&b.last_updated))
                    .reverse()
            })
            .cloned()
    }

    /// Get the best-known model for a task type, ranked by realized success rate.
    ///
    /// This consumes the per-task performance that `update_model_performance`
    /// records into `ModelPerformance::task_types` — previously that breakdown was
    /// written but never read by any decision path. Returns the `(model_name,
    /// success_rate)` with the highest realized success rate for `task_type`,
    /// requiring at least `min_uses` observations so a single lucky/failed run does
    /// not dominate. Returns `None` when no model clears the threshold.
    pub fn best_model_for_task(&self, task_type: &str, min_uses: usize) -> Option<(String, f64)> {
        let performance = self.model_performance.lock().ok()?;

        performance
            .values()
            .filter_map(|model| {
                model.task_types.get(task_type).and_then(|task| {
                    if task.uses >= min_uses {
                        Some((model.model_name.clone(), task.success_rate))
                    } else {
                        None
                    }
                })
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    /// Get improvement suggestions
    pub fn get_improvement_suggestions(&self) -> Vec<ImprovementSuggestion> {
        self.improvement_suggestions
            .lock()
            .map_or_else(|_| Vec::new(), |suggestions| suggestions.clone())
    }

    /// Analyze flywheel performance
    pub fn analyze(&self) -> FlywheelAnalysis {
        let (Ok(records), Ok(performance), Ok(patterns)) = (
            self.records.lock(),
            self.model_performance.lock(),
            self.routing_patterns.lock(),
        ) else {
            return FlywheelAnalysis {
                total_executions: 0,
                success_rate: 0.0,
                avg_cost: 0.0,
                avg_quality: 0.0,
                models_tracked: 0,
                patterns_discovered: 0,
                suggestions_generated: 0,
            };
        };

        let total_executions = records.len();
        let successful_executions = records.iter().filter(|r| r.outcome.success).count();
        let avg_cost =
            records.iter().map(|r| r.cost.total_cost).sum::<f64>() / total_executions.max(1) as f64;
        let avg_quality = records.iter().map(|r| r.outcome.quality_score).sum::<f64>()
            / total_executions.max(1) as f64;

        FlywheelAnalysis {
            total_executions,
            success_rate: successful_executions as f64 / total_executions.max(1) as f64,
            avg_cost,
            avg_quality,
            models_tracked: performance.len(),
            patterns_discovered: patterns.len(),
            suggestions_generated: self
                .improvement_suggestions
                .lock()
                .map_or(0, |suggestions| suggestions.len()),
        }
    }

    /// Update model performance from execution record
    fn update_model_performance(&self, record: &ExecutionRecord) {
        let Ok(mut performance) = self.model_performance.lock() else {
            return;
        };

        let model_name = &record.routing_decision.selected_model;
        let entry = performance
            .entry(model_name.clone())
            .or_insert(ModelPerformance {
                model_name: model_name.clone(),
                total_uses: 0,
                success_rate: 0.0,
                avg_cost: 0.0,
                avg_latency_ms: 0,
                avg_quality: 0.0,
                task_types: HashMap::new(),
            });

        // Update running averages
        let total = entry.total_uses as f64;
        entry.success_rate = (entry.success_rate * total
            + if record.outcome.success { 1.0 } else { 0.0 })
            / (total + 1.0);
        entry.avg_cost = (entry.avg_cost * total + record.cost.total_cost) / (total + 1.0);
        entry.avg_latency_ms = (entry.avg_latency_ms * total as u64
            + record.execution_trace.total_time_ms)
            / (total as u64 + 1);
        entry.avg_quality =
            (entry.avg_quality * total + record.outcome.quality_score) / (total + 1.0);
        entry.total_uses += 1;

        // Update task-specific performance
        let task_type = self.infer_task_type(&record.query);
        let task_entry = entry
            .task_types
            .entry(task_type.clone())
            .or_insert(TaskPerformance {
                task_type: task_type.clone(),
                uses: 0,
                success_rate: 0.0,
                avg_cost: 0.0,
                avg_latency_ms: 0,
            });

        let task_total = task_entry.uses as f64;
        task_entry.success_rate = (task_entry.success_rate * task_total
            + if record.outcome.success { 1.0 } else { 0.0 })
            / (task_total + 1.0);
        task_entry.avg_cost =
            (task_entry.avg_cost * task_total + record.cost.total_cost) / (task_total + 1.0);
        task_entry.avg_latency_ms = (task_entry.avg_latency_ms * task_total as u64
            + record.execution_trace.total_time_ms)
            / (task_total as u64 + 1);
        task_entry.uses += 1;
    }

    /// Update routing patterns from execution record
    fn update_routing_patterns(&self, record: &ExecutionRecord) {
        let Ok(mut patterns) = self.routing_patterns.lock() else {
            return;
        };

        let features = self.extract_query_features(&record.query);
        let pattern_id = features.join("|");

        let model_name = record.routing_decision.selected_model.clone();
        let realized_quality = record.outcome.quality_score;
        let succeeded = record.outcome.success;

        let pattern = patterns
            .entry(pattern_id.clone())
            .or_insert(RoutingPattern {
                pattern_id: pattern_id.clone(),
                query_features: features.clone(),
                best_model: model_name.clone(),
                confidence: 0.0,
                support_count: 0,
                last_updated: 0,
                model_quality: HashMap::new(),
            });

        // Only successful executions reinforce a pattern.
        if succeeded {
            pattern.support_count += 1;
            // `confidence` is the running mean of *realized* outcome quality for this
            // pattern, not the router's self-reported confidence (which the old code
            // echoed and which carries no information about actual results).
            pattern.confidence = (pattern.confidence * (pattern.support_count - 1) as f64
                + realized_quality)
                / pattern.support_count as f64;

            // Replace the incumbent best_model only when the challenger has beaten
            // it on realized quality. A per-model running mean of realized quality
            // within this pattern is tracked in `model_quality`; previously the
            // incumbent was overwritten on *every* success regardless of which model
            // actually performed better.
            let challenger_mean = {
                let entry =
                    pattern
                        .model_quality
                        .entry(model_name.clone())
                        .or_insert(ModelQuality {
                            uses: 0,
                            mean_quality: 0.0,
                        });
                entry.mean_quality = (entry.mean_quality * entry.uses as f64 + realized_quality)
                    / (entry.uses as f64 + 1.0);
                entry.uses += 1;
                entry.mean_quality
            };

            let incumbent_mean = pattern
                .model_quality
                .get(&pattern.best_model)
                .map_or(0.0, |entry| entry.mean_quality);

            if model_name == pattern.best_model || challenger_mean > incumbent_mean {
                pattern.best_model = model_name;
            }

            pattern.last_updated = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
        }
    }

    /// Generate improvement suggestions from execution record
    fn generate_improvements(&self, record: &ExecutionRecord) {
        let Ok(mut suggestions) = self.improvement_suggestions.lock() else {
            return;
        };

        // Check for high-cost failures
        if !record.outcome.success && record.cost.total_cost > 0.01 {
            suggestions.push(ImprovementSuggestion {
                suggestion_id: format!("cost-optimization-{}", record.record_id),
                category: "cost".to_string(),
                description: format!(
                    "High-cost failure detected: ${:.4} for query '{}'",
                    record.cost.total_cost, record.query
                ),
                expected_improvement: 0.2,
                confidence: 0.8,
                evidence: vec![format!(
                    "Model: {}, Cost: ${:.4}",
                    record.routing_decision.selected_model, record.cost.total_cost
                )],
            });
        }

        // Check for slow executions
        if record.execution_trace.total_time_ms > 5000 {
            suggestions.push(ImprovementSuggestion {
                suggestion_id: format!("latency-optimization-{}", record.record_id),
                category: "latency".to_string(),
                description: format!(
                    "Slow execution detected: {}ms for query '{}'",
                    record.execution_trace.total_time_ms, record.query
                ),
                expected_improvement: 0.3,
                confidence: 0.7,
                evidence: vec![format!(
                    "Model: {}, Latency: {}ms",
                    record.routing_decision.selected_model, record.execution_trace.total_time_ms
                )],
            });
        }

        // Check for low-quality results
        if record.outcome.quality_score < 0.5 && record.outcome.success {
            suggestions.push(ImprovementSuggestion {
                suggestion_id: format!("quality-improvement-{}", record.record_id),
                category: "quality".to_string(),
                description: format!(
                    "Low quality result detected: {:.2} for query '{}'",
                    record.outcome.quality_score, record.query
                ),
                expected_improvement: 0.4,
                confidence: 0.9,
                evidence: vec![format!(
                    "Model: {}, Quality: {:.2}",
                    record.routing_decision.selected_model, record.outcome.quality_score
                )],
            });
        }

        // Limit suggestions
        if suggestions.len() > 100 {
            suggestions.remove(0);
        }
    }

    /// Extract features from query for pattern matching
    fn extract_query_features(&self, query: &str) -> Vec<String> {
        let mut features = Vec::new();
        let query_lower = query.to_lowercase();

        // Task type features
        if query_lower.contains("code")
            || query_lower.contains("function")
            || query_lower.contains("implement")
        {
            features.push("code_generation".to_string());
        }
        if query_lower.contains("analyze")
            || query_lower.contains("review")
            || query_lower.contains("explain")
        {
            features.push("analysis".to_string());
        }
        if query_lower.contains("create")
            || query_lower.contains("build")
            || query_lower.contains("generate")
        {
            features.push("creation".to_string());
        }
        if query_lower.contains("fix")
            || query_lower.contains("debug")
            || query_lower.contains("error")
        {
            features.push("debugging".to_string());
        }

        // Complexity features
        if query.len() > 1000 {
            features.push("long_query".to_string());
        }
        if query_lower.contains("multiple")
            || query_lower.contains("several")
            || query_lower.contains("complex")
        {
            features.push("complex_task".to_string());
        }

        // Tool features
        if query_lower.contains("file")
            || query_lower.contains("read")
            || query_lower.contains("write")
        {
            features.push("file_operations".to_string());
        }
        if query_lower.contains("api")
            || query_lower.contains("call")
            || query_lower.contains("request")
        {
            features.push("api_calls".to_string());
        }

        features
    }

    /// Infer task type from query
    fn infer_task_type(&self, query: &str) -> String {
        let query_lower = query.to_lowercase();

        if query_lower.contains("code")
            || query_lower.contains("function")
            || query_lower.contains("implement")
        {
            "code_generation".to_string()
        } else if query_lower.contains("analyze")
            || query_lower.contains("review")
            || query_lower.contains("explain")
        {
            "analysis".to_string()
        } else if query_lower.contains("create")
            || query_lower.contains("build")
            || query_lower.contains("generate")
        {
            "creation".to_string()
        } else if query_lower.contains("fix")
            || query_lower.contains("debug")
            || query_lower.contains("error")
        {
            "debugging".to_string()
        } else {
            "general".to_string()
        }
    }
}

#[derive(Clone, Debug)]
pub struct FlywheelAnalysis {
    pub total_executions: usize,
    pub success_rate: f64,
    pub avg_cost: f64,
    pub avg_quality: f64,
    pub models_tracked: usize,
    pub patterns_discovered: usize,
    pub suggestions_generated: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_flywheel() {
        let flywheel = DataFlywheel::new(100);

        // Record some executions
        for i in 0..10 {
            flywheel.record_execution(ExecutionRecord {
                record_id: format!("record-{}", i),
                timestamp: 1234567890 + i,
                query: format!("test query {}", i),
                harness_state: HarnessStateSnapshot {
                    available_tools: vec!["test_tool".to_string()],
                    current_budget: BudgetSnapshot {
                        tokens_remaining: 1000,
                        time_remaining_ms: 5000,
                        memory_remaining_mb: 100,
                        tool_calls_remaining: 10,
                    },
                    resource_usage: ResourceUsageSnapshot {
                        cpu_percent: 50.0,
                        memory_mb: 512,
                        active_sessions: 5,
                        queued_tasks: 2,
                    },
                    conversation_context: "test context".to_string(),
                },
                routing_decision: RoutingDecisionSnapshot {
                    selected_model: "test-model".to_string(),
                    confidence: 0.9,
                    reasoning: "test reasoning".to_string(),
                    fallback_models: vec![],
                    policy: "balanced".to_string(),
                },
                execution_trace: ExecutionTraceSnapshot {
                    steps: vec![],
                    total_time_ms: 100 + i * 10,
                    total_tokens: 100,
                    tool_calls: vec![],
                    errors: vec![],
                },
                outcome: OutcomeSnapshot {
                    success: i % 2 == 0,
                    quality_score: 0.8,
                    user_satisfaction: Some(0.9),
                    verification_passed: true,
                    side_effects: vec![],
                },
                cost: CostSnapshot {
                    total_cost: 0.001,
                    cost_per_token: 0.00001,
                    cost_per_tool_call: 0.0001,
                    budget_utilization: 0.1,
                },
                labels: vec!["test".to_string()],
            });
        }

        let analysis = flywheel.analyze();
        assert_eq!(analysis.total_executions, 10);
        assert!(analysis.success_rate > 0.0);
        assert!(analysis.avg_cost > 0.0);
        assert!(analysis.models_tracked > 0);
    }
}
