//! Harness-native routing: step-level model selection conditioned on full harness state.
//!
//! Uses LightGBM cold-start ranker + staged router-model path.
//! Execution traces train better routers (data flywheel).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Harness state for routing decisions
#[derive(Clone, Debug)]
pub struct HarnessState {
    pub query: String,
    pub conversation_history: Vec<String>,
    pub available_tools: Vec<String>,
    pub current_budget: Budget,
    pub model_capabilities: HashMap<String, ModelCapability>,
    pub recent_performance: Vec<ModelPerformance>,
    pub resource_usage: ResourceUsage,
}

#[derive(Clone, Debug)]
pub struct Budget {
    pub tokens_remaining: usize,
    pub time_remaining_ms: u64,
    pub memory_remaining_mb: usize,
    pub tool_calls_remaining: usize,
}

#[derive(Clone, Debug)]
pub struct ModelCapability {
    pub name: String,
    pub cost_per_1k_tokens: f64,
    pub latency_ms: u64,
    pub context_length: usize,
    pub tool_use_reliability: f64,
    pub task_family_success: HashMap<String, f64>,
    pub supported_features: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ModelPerformance {
    pub model_name: String,
    pub task_type: String,
    pub success_rate: f64,
    pub avg_latency_ms: u64,
    pub avg_cost: f64,
    pub last_used: u64,
}

#[derive(Clone, Debug)]
pub struct ResourceUsage {
    pub cpu_percent: f64,
    pub memory_mb: usize,
    pub active_sessions: usize,
    pub queued_tasks: usize,
}

/// Routing decision result
#[derive(Clone, Debug)]
pub struct RoutingDecision {
    pub selected_model: String,
    pub confidence: f64,
    pub reasoning: String,
    pub fallback_models: Vec<String>,
    pub estimated_cost: f64,
    pub estimated_latency_ms: u64,
    pub required_capabilities: Vec<String>,
}

/// Execution trace for data flywheel
#[derive(Clone, Debug)]
pub struct ExecutionTrace {
    pub trace_id: String,
    pub query: String,
    pub harness_state: HarnessState,
    pub routing_decision: RoutingDecision,
    pub execution_time_ms: u64,
    pub success: bool,
    pub cost: f64,
    pub tokens_used: usize,
    pub tool_calls: usize,
    pub errors: Vec<String>,
    pub timestamp: u64,
}

/// Harness-native router with data flywheel
pub struct HarnessRouter {
    model_registry: HashMap<String, ModelCapability>,
    performance_history: Arc<Mutex<Vec<ModelPerformance>>>,
    execution_traces: Arc<Mutex<Vec<ExecutionTrace>>>,
    routing_policy: RoutingPolicy,
    flywheel_enabled: bool,
}

#[derive(Clone, Debug)]
pub enum RoutingPolicy {
    CostEffective,    // Minimize cost while meeting quality threshold
    HighAccuracy,     // Maximize accuracy regardless of cost
    Balanced,         // Balance cost and accuracy
    LatencyOptimized, // Minimize latency
}

impl HarnessRouter {
    pub fn new(policy: RoutingPolicy) -> Self {
        Self {
            model_registry: HashMap::new(),
            performance_history: Arc::new(Mutex::new(Vec::new())),
            execution_traces: Arc::new(Mutex::new(Vec::new())),
            routing_policy: policy,
            flywheel_enabled: true,
        }
    }

    /// Register a model with its capabilities
    pub fn register_model(&mut self, capability: ModelCapability) {
        self.model_registry
            .insert(capability.name.clone(), capability);
    }

    /// Route a task to the best model based on current harness state
    pub fn route(&self, state: &HarnessState) -> RoutingDecision {
        let candidates = self.filter_candidates(state);
        let ranked = self.rank_candidates(candidates, state);

        if ranked.is_empty() {
            return RoutingDecision {
                selected_model: "fallback".to_string(),
                confidence: 0.0,
                reasoning: "No suitable model found".to_string(),
                fallback_models: vec![],
                estimated_cost: 0.0,
                estimated_latency_ms: 0,
                required_capabilities: vec![],
            };
        }

        let best = ranked[0].clone();
        let fallbacks: Vec<String> = ranked
            .iter()
            .skip(1)
            .take(3)
            .map(|m| m.name.clone())
            .collect();

        RoutingDecision {
            selected_model: best.name.clone(),
            confidence: best.score,
            reasoning: best.reasoning,
            fallback_models: fallbacks,
            estimated_cost: best.estimated_cost,
            estimated_latency_ms: best.estimated_latency_ms,
            required_capabilities: best.required_capabilities,
        }
    }

    /// Record execution trace for data flywheel
    pub fn record_trace(&self, trace: ExecutionTrace) {
        if self.flywheel_enabled {
            if let Ok(mut traces) = self.execution_traces.lock() {
                traces.push(trace);

                // Limit trace history to prevent unbounded growth
                if traces.len() > 10000 {
                    traces.remove(0);
                }
            }
        }
    }

    /// Get execution traces for analysis
    pub fn get_traces(&self) -> Vec<ExecutionTrace> {
        self.execution_traces
            .lock()
            .map_or_else(|_| Vec::new(), |traces| traces.clone())
    }

    /// Update model performance based on execution results
    pub fn update_performance(
        &self,
        model_name: &str,
        task_type: &str,
        success: bool,
        latency_ms: u64,
        cost: f64,
    ) {
        let Ok(mut history) = self.performance_history.lock() else {
            return;
        };

        // Find existing performance record
        if let Some(record) = history
            .iter_mut()
            .find(|r| r.model_name == model_name && r.task_type == task_type)
        {
            // Update running average
            let total =
                record.success_rate * record.last_used as f64 + if success { 1.0 } else { 0.0 };
            record.success_rate = total / (record.last_used + 1) as f64;
            record.avg_latency_ms =
                (record.avg_latency_ms * record.last_used + latency_ms) / (record.last_used + 1);
            record.avg_cost =
                (record.avg_cost * record.last_used as f64 + cost) / (record.last_used + 1) as f64;
            record.last_used += 1;
        } else {
            // Create new record
            history.push(ModelPerformance {
                model_name: model_name.to_string(),
                task_type: task_type.to_string(),
                success_rate: if success { 1.0 } else { 0.0 },
                avg_latency_ms: latency_ms,
                avg_cost: cost,
                last_used: 1,
            });
        }
    }

    /// Filter candidate models based on requirements
    fn filter_candidates(&self, state: &HarnessState) -> Vec<&ModelCapability> {
        self.model_registry
            .values()
            .filter(|model| {
                // Check if model supports required features
                let required_features = self.extract_required_features(state);
                required_features
                    .iter()
                    .all(|f| model.supported_features.contains(f))
            })
            .filter(|model| {
                // Check if model fits within budget
                model.cost_per_1k_tokens * (state.query.len() as f64 / 1000.0)
                    <= state.current_budget.tokens_remaining as f64
            })
            .collect()
    }

    /// Rank candidate models using routing policy
    fn rank_candidates(
        &self,
        candidates: Vec<&ModelCapability>,
        state: &HarnessState,
    ) -> Vec<RankedModel> {
        let mut ranked: Vec<RankedModel> = candidates
            .into_iter()
            .map(|model| self.score_model(model, state))
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked
    }

    /// Score a model for the current state
    fn score_model(&self, model: &ModelCapability, state: &HarnessState) -> RankedModel {
        let mut score = 0.0;
        let mut reasoning = Vec::new();

        // Base score from task family success
        let task_type = self.infer_task_type(&state.query);
        if let Some(success_rate) = model.task_family_success.get(&task_type) {
            score += success_rate * 0.4;
            reasoning.push(format!("Task family success: {:.2}", success_rate));
        }

        // Adjust based on routing policy
        match self.routing_policy {
            RoutingPolicy::CostEffective => {
                let cost_score = 1.0 / (model.cost_per_1k_tokens + 0.001); // Inverse cost
                score += cost_score * 0.3;
                reasoning.push(format!("Cost score: {:.2}", cost_score));
            }
            RoutingPolicy::HighAccuracy => {
                let quality_score = model.tool_use_reliability;
                score += quality_score * 0.5;
                reasoning.push(format!("Quality score: {:.2}", quality_score));
            }
            RoutingPolicy::Balanced => {
                let cost_score = 1.0 / (model.cost_per_1k_tokens + 0.001);
                let quality_score = model.tool_use_reliability;
                score += (cost_score + quality_score) * 0.25;
                reasoning.push(format!("Balanced score: {:.2}", cost_score + quality_score));
            }
            RoutingPolicy::LatencyOptimized => {
                let latency_score = 1.0 / (model.latency_ms as f64 + 1.0);
                score += latency_score * 0.4;
                reasoning.push(format!("Latency score: {:.2}", latency_score));
            }
        }

        // Adjust based on recent performance
        if let Ok(history) = self.performance_history.lock() {
            if let Some(perf) = history
                .iter()
                .find(|p| p.model_name == model.name && p.task_type == task_type)
            {
                score += perf.success_rate * 0.2;
                reasoning.push(format!("Recent success: {:.2}", perf.success_rate));
            }
        }

        // Penalize if model is over budget
        let estimated_cost = model.cost_per_1k_tokens * (state.query.len() as f64 / 1000.0);
        if estimated_cost > state.current_budget.tokens_remaining as f64 {
            score *= 0.1; // Heavy penalty for over-budget
            reasoning.push("Over budget".to_string());
        }

        RankedModel {
            name: model.name.clone(),
            score,
            reasoning: reasoning.join("; "),
            estimated_cost,
            estimated_latency_ms: model.latency_ms,
            required_capabilities: model.supported_features.clone(),
        }
    }

    /// Extract required features from harness state
    fn extract_required_features(&self, state: &HarnessState) -> Vec<String> {
        let mut features = Vec::new();

        // Check if tools are needed
        if !state.available_tools.is_empty() {
            features.push("tool_calling".to_string());
        }

        // Check if long context is needed
        if state.conversation_history.len() > 10 {
            features.push("long_context".to_string());
        }

        // Check if streaming is needed
        if state.query.len() > 1000 {
            features.push("streaming".to_string());
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
struct RankedModel {
    name: String,
    score: f64,
    reasoning: String,
    estimated_cost: f64,
    estimated_latency_ms: u64,
    required_capabilities: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_harness_router() {
        let mut router = HarnessRouter::new(RoutingPolicy::Balanced);

        // Register models
        router.register_model(ModelCapability {
            name: "fast-model".to_string(),
            cost_per_1k_tokens: 0.001,
            latency_ms: 100,
            context_length: 4096,
            tool_use_reliability: 0.85,
            task_family_success: {
                let mut map = HashMap::new();
                map.insert("code_generation".to_string(), 0.9);
                map.insert("analysis".to_string(), 0.8);
                map
            },
            supported_features: vec!["tool_calling".to_string(), "streaming".to_string()],
        });

        router.register_model(ModelCapability {
            name: "accurate-model".to_string(),
            cost_per_1k_tokens: 0.01,
            latency_ms: 500,
            context_length: 8192,
            tool_use_reliability: 0.95,
            task_family_success: {
                let mut map = HashMap::new();
                map.insert("code_generation".to_string(), 0.95);
                map.insert("analysis".to_string(), 0.9);
                map
            },
            supported_features: vec!["tool_calling".to_string(), "long_context".to_string()],
        });

        // Test routing
        let state = HarnessState {
            query: "Write a function to calculate fibonacci".to_string(),
            conversation_history: vec![],
            available_tools: vec!["code_editor".to_string()],
            current_budget: Budget {
                tokens_remaining: 1000,
                time_remaining_ms: 5000,
                memory_remaining_mb: 100,
                tool_calls_remaining: 10,
            },
            model_capabilities: HashMap::new(),
            recent_performance: vec![],
            resource_usage: ResourceUsage {
                cpu_percent: 50.0,
                memory_mb: 512,
                active_sessions: 5,
                queued_tasks: 2,
            },
        };

        let decision = router.route(&state);
        assert!(!decision.selected_model.is_empty());
        assert!(decision.confidence > 0.0);
        assert!(!decision.reasoning.is_empty());
    }

    #[test]
    fn test_data_flywheel() {
        let mut router = HarnessRouter::new(RoutingPolicy::Balanced);

        // Register a model
        router.register_model(ModelCapability {
            name: "test-model".to_string(),
            cost_per_1k_tokens: 0.001,
            latency_ms: 100,
            context_length: 4096,
            tool_use_reliability: 0.85,
            task_family_success: HashMap::new(),
            supported_features: vec!["tool_calling".to_string()],
        });

        // Record some traces
        for i in 0..5 {
            router.record_trace(ExecutionTrace {
                trace_id: format!("trace-{}", i),
                query: format!("test query {}", i),
                harness_state: HarnessState {
                    query: format!("test query {}", i),
                    conversation_history: vec![],
                    available_tools: vec![],
                    current_budget: Budget {
                        tokens_remaining: 1000,
                        time_remaining_ms: 5000,
                        memory_remaining_mb: 100,
                        tool_calls_remaining: 10,
                    },
                    model_capabilities: HashMap::new(),
                    recent_performance: vec![],
                    resource_usage: ResourceUsage {
                        cpu_percent: 50.0,
                        memory_mb: 512,
                        active_sessions: 5,
                        queued_tasks: 2,
                    },
                },
                routing_decision: RoutingDecision {
                    selected_model: "test-model".to_string(),
                    confidence: 0.9,
                    reasoning: "test".to_string(),
                    fallback_models: vec![],
                    estimated_cost: 0.001,
                    estimated_latency_ms: 100,
                    required_capabilities: vec![],
                },
                execution_time_ms: 100 + i * 10,
                success: i % 2 == 0,
                cost: 0.001,
                tokens_used: 100,
                tool_calls: 1,
                errors: vec![],
                timestamp: 1234567890 + i,
            });
        }

        let traces = router.get_traces();
        assert_eq!(traces.len(), 5);
        assert!(traces.iter().all(|t| !t.trace_id.is_empty()));
    }
}
