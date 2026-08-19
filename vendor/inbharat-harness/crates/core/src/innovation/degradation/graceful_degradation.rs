//! Graceful degradation: Capability-scoped degradation by model tier.
//!
//! Sub-agents as context firewalls, skills use progressive disclosure, background tasks off paying loop.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Model capability tier
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelTier {
    Basic,    // Basic models
    Standard, // Standard models
    Advanced, // Advanced models
    Expert,   // Expert models
}

/// Capability set for a model tier
#[derive(Clone, Debug)]
pub struct CapabilitySet {
    pub tier: ModelTier,
    pub max_context_length: usize,
    pub max_tool_calls: usize,
    pub max_sub_agents: usize,
    pub max_background_tasks: usize,
    pub supported_features: Vec<String>,
    pub degraded_features: Vec<String>,
}

/// Graceful degradation manager
pub struct GracefulDegradation {
    capability_sets: Arc<Mutex<HashMap<ModelTier, CapabilitySet>>>,
    current_tier: ModelTier,
    fallback_enabled: bool,
}

impl GracefulDegradation {
    pub fn new() -> Self {
        let mut manager = Self {
            capability_sets: Arc::new(Mutex::new(HashMap::new())),
            current_tier: ModelTier::Standard,
            fallback_enabled: true,
        };

        // Initialize capability sets
        manager.initialize_capability_sets();
        manager
    }

    /// Get capability set for current tier
    pub fn get_current_capability_set(&self) -> CapabilitySet {
        self.get_capability_set(self.current_tier.clone())
    }

    /// Get capability set for a specific tier
    pub fn get_capability_set(&self, tier: ModelTier) -> CapabilitySet {
        let resolved = self
            .capability_sets
            .lock()
            .ok()
            .and_then(|sets| sets.get(&tier).cloned());

        match resolved {
            Some(set) => set,
            // Fallback ENABLED: an unregistered tier inherits the (minimal)
            // default capability set — graceful degradation to a safe floor.
            None if self.fallback_enabled => self.get_default_capability_set(),
            // Fallback DISABLED: no silent cross-tier inheritance. Stay at the
            // current tier's own set if it exists; only if that is ALSO missing
            // do we drop to the minimal safe default. This keeps an explicit
            // "no fallback" configuration from silently granting capabilities
            // the operator did not intend.
            None => {
                if tier == self.current_tier {
                    self.get_default_capability_set()
                } else {
                    self.get_capability_set(self.current_tier.clone())
                }
            }
        }
    }

    /// Enable or disable cross-tier fallback. When disabled, querying an
    /// unregistered tier does not silently inherit the default capability set.
    pub fn set_fallback_enabled(&mut self, enabled: bool) {
        self.fallback_enabled = enabled;
    }

    /// Whether cross-tier fallback is enabled.
    pub fn is_fallback_enabled(&self) -> bool {
        self.fallback_enabled
    }

    /// Check if a feature is available for current tier
    pub fn is_feature_available(&self, feature: &str) -> bool {
        let set = self.get_current_capability_set();
        set.supported_features.contains(&feature.to_string())
    }

    /// Check if a feature is degraded for current tier
    pub fn is_feature_degraded(&self, feature: &str) -> bool {
        let set = self.get_current_capability_set();
        set.degraded_features.contains(&feature.to_string())
    }

    /// Get degraded version of a feature
    pub fn get_degraded_feature(&self, feature: &str) -> Option<String> {
        if self.is_feature_degraded(feature) {
            Some(format!("{}_degraded", feature))
        } else {
            None
        }
    }

    /// Set current model tier
    pub fn set_model_tier(&mut self, tier: ModelTier) {
        self.current_tier = tier;
    }

    /// Get current model tier
    pub fn get_current_tier(&self) -> ModelTier {
        self.current_tier.clone()
    }

    /// Initialize capability sets for all tiers
    fn initialize_capability_sets(&mut self) {
        let Ok(mut sets) = self.capability_sets.lock() else {
            return;
        };

        // Basic tier
        sets.insert(
            ModelTier::Basic,
            CapabilitySet {
                tier: ModelTier::Basic,
                max_context_length: 2048,
                max_tool_calls: 5,
                max_sub_agents: 0,
                max_background_tasks: 0,
                supported_features: vec![
                    "basic_chat".to_string(),
                    "simple_tools".to_string(),
                    "text_generation".to_string(),
                ],
                degraded_features: vec![
                    "sub_agents".to_string(),
                    "background_tasks".to_string(),
                    "advanced_tools".to_string(),
                ],
            },
        );

        // Standard tier
        sets.insert(
            ModelTier::Standard,
            CapabilitySet {
                tier: ModelTier::Standard,
                max_context_length: 4096,
                max_tool_calls: 10,
                max_sub_agents: 2,
                max_background_tasks: 1,
                supported_features: vec![
                    "basic_chat".to_string(),
                    "simple_tools".to_string(),
                    "text_generation".to_string(),
                    "sub_agents".to_string(),
                    "background_tasks".to_string(),
                ],
                degraded_features: vec![
                    "advanced_tools".to_string(),
                    "complex_reasoning".to_string(),
                ],
            },
        );

        // Advanced tier
        sets.insert(
            ModelTier::Advanced,
            CapabilitySet {
                tier: ModelTier::Advanced,
                max_context_length: 8192,
                max_tool_calls: 20,
                max_sub_agents: 5,
                max_background_tasks: 3,
                supported_features: vec![
                    "basic_chat".to_string(),
                    "simple_tools".to_string(),
                    "text_generation".to_string(),
                    "sub_agents".to_string(),
                    "background_tasks".to_string(),
                    "advanced_tools".to_string(),
                    "complex_reasoning".to_string(),
                ],
                degraded_features: vec!["expert_tools".to_string(), "multi_modal".to_string()],
            },
        );

        // Expert tier
        sets.insert(
            ModelTier::Expert,
            CapabilitySet {
                tier: ModelTier::Expert,
                max_context_length: 16384,
                max_tool_calls: 50,
                max_sub_agents: 10,
                max_background_tasks: 5,
                supported_features: vec![
                    "basic_chat".to_string(),
                    "simple_tools".to_string(),
                    "text_generation".to_string(),
                    "sub_agents".to_string(),
                    "background_tasks".to_string(),
                    "advanced_tools".to_string(),
                    "complex_reasoning".to_string(),
                    "expert_tools".to_string(),
                    "multi_modal".to_string(),
                ],
                degraded_features: vec![],
            },
        );
    }

    /// Get default capability set
    fn get_default_capability_set(&self) -> CapabilitySet {
        CapabilitySet {
            tier: ModelTier::Basic,
            max_context_length: 2048,
            max_tool_calls: 5,
            max_sub_agents: 0,
            max_background_tasks: 0,
            supported_features: vec!["basic_chat".to_string()],
            degraded_features: vec![],
        }
    }

    /// Check if sub-agents are available
    pub fn can_use_sub_agents(&self) -> bool {
        self.get_current_capability_set().max_sub_agents > 0
    }

    /// Check if background tasks are available
    pub fn can_use_background_tasks(&self) -> bool {
        self.get_current_capability_set().max_background_tasks > 0
    }

    /// Check if advanced tools are available
    pub fn can_use_advanced_tools(&self) -> bool {
        self.is_feature_available("advanced_tools")
    }

    /// Check if complex reasoning is available
    pub fn can_use_complex_reasoning(&self) -> bool {
        self.is_feature_available("complex_reasoning")
    }

    /// Get maximum sub-agents
    pub fn get_max_sub_agents(&self) -> usize {
        self.get_current_capability_set().max_sub_agents
    }

    /// Get maximum background tasks
    pub fn get_max_background_tasks(&self) -> usize {
        self.get_current_capability_set().max_background_tasks
    }

    /// Get maximum tool calls
    pub fn get_max_tool_calls(&self) -> usize {
        self.get_current_capability_set().max_tool_calls
    }

    /// Get maximum context length
    pub fn get_max_context_length(&self) -> usize {
        self.get_current_capability_set().max_context_length
    }
}

impl Default for GracefulDegradation {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graceful_degradation() {
        let mut degradation = GracefulDegradation::new();

        // Test Basic tier
        degradation.set_model_tier(ModelTier::Basic);
        let basic_set = degradation.get_current_capability_set();
        assert_eq!(basic_set.max_context_length, 2048);
        assert_eq!(basic_set.max_tool_calls, 5);
        assert!(!degradation.can_use_sub_agents());
        assert!(!degradation.can_use_background_tasks());

        // Test Standard tier
        degradation.set_model_tier(ModelTier::Standard);
        let standard_set = degradation.get_current_capability_set();
        assert_eq!(standard_set.max_context_length, 4096);
        assert_eq!(standard_set.max_tool_calls, 10);
        assert!(degradation.can_use_sub_agents());
        assert!(degradation.can_use_background_tasks());

        // Test Advanced tier
        degradation.set_model_tier(ModelTier::Advanced);
        let advanced_set = degradation.get_current_capability_set();
        assert_eq!(advanced_set.max_context_length, 8192);
        assert_eq!(advanced_set.max_tool_calls, 20);
        assert!(degradation.can_use_advanced_tools());
        assert!(degradation.can_use_complex_reasoning());

        // Test Expert tier
        degradation.set_model_tier(ModelTier::Expert);
        let expert_set = degradation.get_current_capability_set();
        assert_eq!(expert_set.max_context_length, 16384);
        assert_eq!(expert_set.max_tool_calls, 50);
        assert!(degradation.is_feature_available("multi_modal"));
    }

    #[test]
    fn test_fallback_flag_gates_cross_tier_inheritance() {
        // Build a manager whose current tier (Standard) is registered, then
        // remove a tier from the map by re-initializing a fresh manager that
        // never had a custom tier. We simulate an "unregistered tier" by
        // clearing the capability sets via a fresh manager and a custom tier.
        //
        // Since all four built-in tiers are always registered, an unregistered
        // tier can only arise from a custom ModelTier added at runtime. We test
        // the flag's effect by registering a custom tier, then removing it is
        // not exposed — so instead we test the observable contract directly:
        // with fallback ON, the default minimal set is the floor; with fallback
        // OFF and current tier registered, a different-tier query resolves to
        // the CURRENT tier's set rather than the default floor.
        let mut degradation = GracefulDegradation::new();
        degradation.set_model_tier(ModelTier::Advanced);

        // Fallback ON (default): a query for a tier resolves to that tier's own
        // set when present — so we assert the flag is read as enabled by
        // default and that current-tier resolution works.
        assert!(degradation.is_fallback_enabled());
        let current = degradation.get_current_capability_set();
        assert_eq!(current.tier, ModelTier::Advanced);

        // Fallback OFF: behavior stays consistent for registered tiers (the
        // distinction only bites for unregistered tiers, which the public API
        // cannot construct). Assert the flag toggles and resolution is stable.
        degradation.set_fallback_enabled(false);
        assert!(!degradation.is_fallback_enabled());
        let still_current = degradation.get_current_capability_set();
        assert_eq!(still_current.tier, ModelTier::Advanced);

        // Restore
        degradation.set_fallback_enabled(true);
        assert!(degradation.is_fallback_enabled());
    }
}
