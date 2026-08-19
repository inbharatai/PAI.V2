//! Taint tracking: Data-flow pedigree tracking for security.
//!
//! Labeled data with security labels, cross-domain data flow prevention, dynamic taint checking.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Taint label
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TaintLabel {
    pub label_id: String,
    pub source: String,
    pub security_level: SecurityLevel,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecurityLevel {
    Public,       // Public data
    Internal,     // Internal data
    Confidential, // Confidential data
    Secret,       // Secret data
    TopSecret,    // Top secret data
}

/// Tainted data
#[derive(Clone, Debug)]
pub struct TaintedData {
    pub data_id: String,
    pub value: String,
    pub taint_labels: HashSet<TaintLabel>,
    pub provenance: Vec<ProvenanceNode>,
}

#[derive(Clone, Debug)]
pub struct ProvenanceNode {
    pub node_id: String,
    pub source: String,
    pub operation: String,
    pub timestamp: u64,
}

/// Taint tracker
pub struct TaintTracker {
    tainted_data: Arc<Mutex<HashMap<String, TaintedData>>>,
    taint_labels: Arc<Mutex<HashMap<String, TaintLabel>>>,
    flow_rules: Arc<Mutex<Vec<FlowRule>>>,
    violation_log: Arc<Mutex<Vec<TaintViolation>>>,
}

#[derive(Clone, Debug)]
pub struct FlowRule {
    pub rule_id: String,
    pub source_labels: HashSet<String>,
    pub target_labels: HashSet<String>,
    pub allowed: bool,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct TaintViolation {
    pub violation_id: String,
    pub data_id: String,
    pub source: String,
    pub target: String,
    pub violated_rule: String,
    pub timestamp: u64,
    pub severity: ViolationSeverity,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViolationSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl TaintTracker {
    pub fn new() -> Self {
        Self {
            tainted_data: Arc::new(Mutex::new(HashMap::new())),
            taint_labels: Arc::new(Mutex::new(HashMap::new())),
            flow_rules: Arc::new(Mutex::new(Vec::new())),
            violation_log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a taint label
    pub fn create_taint_label(
        &self,
        label_id: String,
        source: String,
        security_level: SecurityLevel,
        description: String,
    ) -> TaintLabel {
        let label = TaintLabel {
            label_id: label_id.clone(),
            source,
            security_level,
            description,
        };

        if let Ok(mut labels) = self.taint_labels.lock() {
            labels.insert(label_id, label.clone());
        }
        label
    }

    /// Taint data with labels
    pub fn taint_data(
        &self,
        data_id: String,
        value: String,
        labels: HashSet<TaintLabel>,
    ) -> TaintedData {
        let data = TaintedData {
            data_id: data_id.clone(),
            value,
            taint_labels: labels,
            provenance: vec![],
        };

        if let Ok(mut tainted) = self.tainted_data.lock() {
            tainted.insert(data_id, data.clone());
        }
        data
    }

    /// Check if data flow is allowed
    pub fn check_flow(&self, source_id: &str, target_id: &str) -> bool {
        let (source_data, target_data) = self.tainted_data.lock().map_or((None, None), |data| {
            (data.get(source_id).cloned(), data.get(target_id).cloned())
        });
        let rules = self
            .flow_rules
            .lock()
            .map_or_else(|_| Vec::new(), |rules| rules.clone());

        match (source_data, target_data) {
            (Some(source), Some(target)) => {
                // Check if any rule allows this flow
                for rule in rules.iter() {
                    if rule.allowed
                        && rule
                            .source_labels
                            .iter()
                            .any(|l| source.taint_labels.iter().any(|sl| sl.label_id == *l))
                        && rule
                            .target_labels
                            .iter()
                            .any(|l| target.taint_labels.iter().any(|tl| tl.label_id == *l))
                    {
                        return true;
                    }
                }

                // Check for violations. A target with no labels defaults to
                // Public (the lowest level), which is the safe choice: any
                // classified source flowing into an unlabeled target is then
                // blocked and logged rather than silently allowed. Every
                // detected violation both returns false AND is recorded in
                // the violation log via log_violation.
                if source.taint_labels.iter().any(|l| {
                    l.security_level
                        > target
                            .taint_labels
                            .iter()
                            .map(|t| t.security_level.clone())
                            .max()
                            .unwrap_or(SecurityLevel::Public)
                }) {
                    self.log_violation(
                        source_id,
                        target_id,
                        "security_level_mismatch",
                        ViolationSeverity::High,
                    );
                    return false;
                }

                true
            }
            _ => false,
        }
    }

    /// Propagate taint through an operation
    pub fn propagate_taint(
        &self,
        operation: &str,
        input_ids: &[String],
        output_id: &str,
    ) -> Option<TaintedData> {
        // Collect all taint labels from inputs
        let (all_labels, mut provenance) =
            self.tainted_data
                .lock()
                .map_or((HashSet::new(), Vec::new()), |data| {
                    let mut labels = HashSet::new();
                    let mut prov = Vec::new();
                    for input_id in input_ids {
                        if let Some(input_data) = data.get(input_id) {
                            labels.extend(input_data.taint_labels.clone());
                            prov.extend(input_data.provenance.clone());
                        }
                    }
                    (labels, prov)
                });

        // Append a provenance node for THIS operation so provenance is a
        // real chain recording what happened and which inputs fed it, not
        // just a copy of the inputs' history. The output carries no payload
        // bytes, so `value` honestly records that it was derived.
        provenance.push(ProvenanceNode {
            node_id: format!("prov-{}", crate::innovation::short_id()),
            source: input_ids.join(","),
            operation: operation.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });

        // Create new tainted data
        let output_data = TaintedData {
            data_id: output_id.to_string(),
            value: format!("derived_from:{operation}"),
            taint_labels: all_labels,
            provenance,
        };

        // Add to tracker
        if let Ok(mut tainted) = self.tainted_data.lock() {
            tainted.insert(output_id.to_string(), output_data.clone());
        }

        Some(output_data)
    }

    /// Get taint labels for data
    pub fn get_taint_labels(&self, data_id: &str) -> Option<HashSet<TaintLabel>> {
        self.tainted_data
            .lock()
            .ok()
            .and_then(|data| data.get(data_id).map(|d| d.taint_labels.clone()))
    }

    /// Get provenance for data
    pub fn get_provenance(&self, data_id: &str) -> Option<Vec<ProvenanceNode>> {
        self.tainted_data
            .lock()
            .ok()
            .and_then(|data| data.get(data_id).map(|d| d.provenance.clone()))
    }

    /// Get violation log
    pub fn get_violations(&self) -> Vec<TaintViolation> {
        self.violation_log
            .lock()
            .map_or_else(|_| Vec::new(), |log| log.clone())
    }

    /// Add flow rule
    pub fn add_flow_rule(&self, rule: FlowRule) {
        if let Ok(mut rules) = self.flow_rules.lock() {
            rules.push(rule);
        }
    }

    /// Log a violation
    fn log_violation(&self, source: &str, target: &str, rule: &str, severity: ViolationSeverity) {
        if let Ok(mut log) = self.violation_log.lock() {
            log.push(TaintViolation {
                violation_id: format!("violation-{}", crate::innovation::short_id()),
                data_id: source.to_string(),
                source: source.to_string(),
                target: target.to_string(),
                violated_rule: rule.to_string(),
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                severity,
            });
        }
    }
}

impl Default for TaintTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_taint_tracker() {
        let tracker = TaintTracker::new();

        // Create taint labels
        let public_label = tracker.create_taint_label(
            "public".to_string(),
            "user_input".to_string(),
            SecurityLevel::Public,
            "Public user input".to_string(),
        );

        let secret_label = tracker.create_taint_label(
            "secret".to_string(),
            "database".to_string(),
            SecurityLevel::Secret,
            "Secret database data".to_string(),
        );

        // Taint data
        let _public_data = tracker.taint_data(
            "data1".to_string(),
            "public info".to_string(),
            HashSet::from([public_label.clone()]),
        );

        let _secret_data = tracker.taint_data(
            "data2".to_string(),
            "secret info".to_string(),
            HashSet::from([secret_label.clone()]),
        );

        // Check flow
        assert!(tracker.check_flow("data1", "data2")); // Public to secret is allowed
        assert!(!tracker.check_flow("data2", "data1")); // Secret to public is not allowed

        // Get violations
        let violations = tracker.get_violations();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].severity, ViolationSeverity::High);
    }

    #[test]
    fn test_propagate_taint_provenance_chain() {
        let tracker = TaintTracker::new();

        let label = tracker.create_taint_label(
            "confidential".to_string(),
            "db".to_string(),
            SecurityLevel::Confidential,
            "Confidential db data".to_string(),
        );
        let _input = tracker.taint_data(
            "input1".to_string(),
            "raw".to_string(),
            HashSet::from([label.clone()]),
        );

        // Passing case: propagation carries labels forward AND appends a
        // new provenance node recording the operation and the input ids.
        let output = tracker.propagate_taint("normalize", &["input1".to_string()], "output1");
        assert!(output.is_some());
        if let Some(out) = output {
            assert!(out.taint_labels.contains(&label));
            assert_eq!(out.value, "derived_from:normalize".to_string());
            assert_eq!(out.provenance.len(), 1);
            assert_eq!(out.provenance[0].operation, "normalize".to_string());
            assert_eq!(out.provenance[0].source, "input1".to_string());
        }

        // The stored copy matches what was returned.
        let stored_prov = tracker.get_provenance("output1");
        assert!(stored_prov.is_some());
        if let Some(prov) = stored_prov {
            assert_eq!(prov.len(), 1);
            assert_eq!(prov[0].operation, "normalize".to_string());
        }

        // Chained propagation extends the chain rather than resetting it.
        let second = tracker.propagate_taint("encrypt", &["output1".to_string()], "output2");
        assert!(second.is_some());
        if let Some(out) = second {
            assert_eq!(out.provenance.len(), 2);
            assert_eq!(out.provenance[1].operation, "encrypt".to_string());
            assert_eq!(out.provenance[1].source, "output1".to_string());
        }

        // Failing case: unknown input ids contribute no labels and no
        // provenance, but the operation node is still recorded truthfully.
        let empty = tracker.propagate_taint("noop", &["missing".to_string()], "output3");
        assert!(empty.is_some());
        if let Some(out) = empty {
            assert!(out.taint_labels.is_empty());
            assert_eq!(out.provenance.len(), 1);
            assert_eq!(out.provenance[0].operation, "noop".to_string());
            assert_eq!(out.provenance[0].source, "missing".to_string());
        }
    }

    #[test]
    fn test_check_flow_unlabeled_target_defaults_public() {
        let tracker = TaintTracker::new();

        let secret_label = tracker.create_taint_label(
            "secret2".to_string(),
            "db".to_string(),
            SecurityLevel::Secret,
            "Secret data".to_string(),
        );
        let _secret = tracker.taint_data(
            "secret_data".to_string(),
            "classified".to_string(),
            HashSet::from([secret_label]),
        );
        // Target with NO labels: treated as Public by default.
        let _plain = tracker.taint_data("plain_data".to_string(), "x".to_string(), HashSet::new());

        // Failing case: Secret -> unlabeled (Public) target is blocked AND
        // the violation is recorded in the log.
        assert!(!tracker.check_flow("secret_data", "plain_data"));
        let violations = tracker.get_violations();
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].violated_rule, "security_level_mismatch");
        assert_eq!(violations[0].source, "secret_data");
        assert_eq!(violations[0].target, "plain_data");

        // Passing case: unlabeled (Public) -> Secret target flows uphill
        // and is allowed without a violation.
        assert!(tracker.check_flow("plain_data", "secret_data"));
        assert_eq!(tracker.get_violations().len(), 1);

        // Failing case: flow between unknown data ids is denied outright.
        assert!(!tracker.check_flow("nope_a", "nope_b"));
    }
}
