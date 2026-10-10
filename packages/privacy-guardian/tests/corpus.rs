//! Blinded, versioned, AUTHORED fixture corpus. Measures harmful misses and false alarms and
//! compares them with the recorded metrics file so any drift is visible, never hidden.
use serde_json::Value;
use unoone_privacy_guardian::*;

fn corpus() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/v1/corpus.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn context(base: &Value, item: &Value) -> Context {
    let s = |v: &Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    Context {
        known_contacts: s(&base["known_contacts"]),
        trusted_domains: s(&base["trusted_domains"]),
        prior_payees: serde_json::from_value(base["prior_payees"].clone()).unwrap(),
        sender: if item["sender"].is_null() {
            None
        } else {
            Some(serde_json::from_value(item["sender"].clone()).unwrap())
        },
        message: if item["message"].is_null() {
            None
        } else {
            Some(serde_json::from_value(item["message"].clone()).unwrap())
        },
        consented_connectors: vec![],
        parent_tools: vec!["model.respond".into()],
    }
}

fn sev(s: &str) -> Severity {
    match s {
        "ALLOW" => Severity::Allow,
        "WARN" => Severity::Warn,
        _ => Severity::Block,
    }
}

#[test]
fn corpus_v1_metrics_are_measured_and_match_recorded_numbers() {
    let c = corpus();
    assert_eq!(c["version"], 1);
    assert_eq!(
        c["authored"], true,
        "corpus must be authored, not real victims"
    );
    let items = c["items"].as_array().unwrap();
    assert_eq!(items.len(), 60);
    let (
        mut scams,
        mut legit,
        mut harmful_misses,
        mut under_severity,
        mut false_alarms,
        mut tolerated,
        mut legit_blocked,
    ) = (0, 0, 0, 0, 0, 0, 0);
    let mut detail = Vec::new();
    for item in items {
        let intent: Intent = serde_json::from_value(item["intent"].clone())
            .unwrap_or_else(|e| panic!("{}: {e}", item["id"]));
        let ctx = context(&c["context"], item);
        let d = check(&intent, &ctx);
        let expected = sev(item["expected_minimum"].as_str().unwrap());
        let id = item["id"].as_str().unwrap();
        match item["label"].as_str().unwrap() {
            "SCAM" => {
                scams += 1;
                if d.severity == Severity::Allow {
                    harmful_misses += 1;
                    detail.push(format!("HARMFUL MISS {id}: {:?}", d.signals));
                } else if d.severity < expected {
                    under_severity += 1;
                    detail.push(format!(
                        "UNDER-SEVERITY {id}: got {:?} expected {:?}",
                        d.severity, expected
                    ));
                }
            }
            _ => {
                legit += 1;
                if d.severity == Severity::Block {
                    legit_blocked += 1;
                    detail.push(format!("LEGIT BLOCKED {id}: {:?}", d.signals));
                } else if d.severity == Severity::Warn && expected == Severity::Warn {
                    tolerated += 1;
                } else if d.severity > expected {
                    false_alarms += 1;
                    detail.push(format!("FALSE ALARM {id}: {:?}", d.signals));
                }
            }
        }
        // Receipts for every item must be masked and bounded.
        let note = match enforce(&d, None, 1) {
            Ok(r) => r.ledger_note(),
            Err(r) => r.receipt.ledger_note(),
        };
        assert!(
            note.len() <= MAX_NOTE_BYTES
                && !note.contains("884213")
                && !note.contains("Hunter22Lane")
                && !note.contains("ya29.a0AfH6SMBx"),
            "{id}"
        );
    }
    let measured = serde_json::json!({
        "corpus_version": 1,
        "guardian_version": GUARDIAN_VERSION,
        "scam_items": scams,
        "legit_items": legit,
        "harmful_misses_allow_on_scam": harmful_misses,
        "under_severity_warn_where_block_expected": under_severity,
        "false_alarms_warn_or_block_on_legit": false_alarms,
        "legit_blocked": legit_blocked,
        "tolerated_reviews_on_legit_by_design": tolerated,
    });
    for line in &detail {
        eprintln!("{line}");
    }
    eprintln!("MEASURED {measured}");
    let recorded: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/corpus/v1/metrics.json"
        ))
        .expect("metrics.json recorded after an actual run"),
    )
    .unwrap();
    assert_eq!(measured, recorded, "corpus metrics drifted from the recorded numbers; re-measure and update metrics.json deliberately");
}

#[test]
fn blinded_ordering_does_not_change_decisions() {
    // Decision is a pure function; shuffling item order or labels cannot influence results.
    let c = corpus();
    let items = c["items"].as_array().unwrap();
    let mut first = Vec::new();
    for item in items {
        let intent: Intent = serde_json::from_value(item["intent"].clone()).unwrap();
        first.push(check(&intent, &context(&c["context"], item)));
    }
    for (i, item) in items.iter().enumerate().rev() {
        let intent: Intent = serde_json::from_value(item["intent"].clone()).unwrap();
        assert_eq!(check(&intent, &context(&c["context"], item)), first[i]);
    }
}

#[test]
fn decisions_golden_matches_recorded_file_for_kotlin_parity() {
    let c = corpus();
    let recorded: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/corpus/v1/decisions.json"
        ))
        .expect("decisions.json emitted by examples/emit_decisions"),
    )
    .unwrap();
    let items = c["items"].as_array().unwrap();
    let golden = recorded["decisions"].as_array().unwrap();
    assert_eq!(items.len(), golden.len());
    for (item, g) in items.iter().zip(golden) {
        let intent: Intent = serde_json::from_value(item["intent"].clone()).unwrap();
        let d = check(&intent, &context(&c["context"], item));
        assert_eq!(g["id"], item["id"]);
        assert_eq!(
            serde_json::to_value(d.severity).unwrap(),
            g["severity"],
            "{}",
            item["id"]
        );
        assert_eq!(
            serde_json::to_value(&d.signals).unwrap(),
            g["signals"],
            "{}",
            item["id"]
        );
        assert_eq!(
            serde_json::to_value(&d.verification_route).unwrap(),
            g["verification_route"],
            "{}",
            item["id"]
        );
        assert_eq!(
            serde_json::to_value(&d.fingerprint).unwrap(),
            g["fingerprint"],
            "{}",
            item["id"]
        );
    }
}
