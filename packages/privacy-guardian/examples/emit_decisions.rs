//! Emits the Rust decisions for corpus v1 as the golden the Kotlin mirror must reproduce.
//! `cargo run -p unoone-privacy-guardian --example emit_decisions --locked --offline > corpus/v1/decisions.json`
use serde_json::{json, Value};
use unoone_privacy_guardian::*;

fn main() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/v1/corpus.json");
    let c: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let s = |v: &Value| {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let base = &c["context"];
    let mut out = Vec::new();
    for item in c["items"].as_array().unwrap() {
        let intent: Intent = serde_json::from_value(item["intent"].clone()).unwrap();
        let ctx = Context {
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
        };
        let d = check(&intent, &ctx);
        out.push(json!({"id": item["id"], "severity": d.severity, "signals": d.signals, "verification_route": d.verification_route, "fingerprint": d.fingerprint}));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"corpus_version": 1, "guardian_version": GUARDIAN_VERSION, "decisions": out})
        )
        .unwrap()
    );
}
