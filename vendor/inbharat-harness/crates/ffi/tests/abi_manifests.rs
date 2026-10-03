//! ABI manifest drift gates: every symbol declared in `abi-v1.json` /
//! `abi-v2.json` must be a real `extern "C"` export in this crate AND a
//! declared function in `include/inbharat_harness.h`, and every function the
//! header declares must appear in a manifest. A symbol that drifts on either
//! side fails CI here instead of surprising a C/Kotlin embedder at link time.
//!
//! Tests return `Result<(), String>` (the workspace convention under the
//! `expect_used`/`unwrap_used` denies): the first broken symbol names itself
//! in the error, no expect/unwrap anywhere.

use inbharat_harness::v2;
use inbharat_harness_core::value::Value;

const ABI_V1: &str = include_str!("../abi-v1.json");
const ABI_V2: &str = include_str!("../abi-v2.json");
const HEADER: &str = include_str!("../include/inbharat_harness.h");
const SOURCE_LIB: &str = include_str!("../src/lib.rs");
const SOURCE_V2: &str = include_str!("../src/v2.rs");

fn string_array(value: &Value, what: &str) -> Result<Vec<String>, String> {
    match value {
        Value::Array(entries) => {
            let mut symbols = Vec::with_capacity(entries.len());
            for entry in entries {
                symbols.push(
                    entry
                        .as_str()
                        .ok_or_else(|| format!("{what} has a non-string entry"))?
                        .to_owned(),
                );
            }
            Ok(symbols)
        }
        other => Err(format!("{what} is not an array: {other:?}")),
    }
}

fn manifest_symbols(manifest: &str, name: &str) -> Result<Vec<String>, String> {
    let parsed = Value::parse_json(manifest)
        .map_err(|message| format!("{name} is not valid bounded JSON: {message}"))?;
    let object = parsed
        .as_object()
        .ok_or_else(|| format!("{name} root is not an object"))?;
    let abi = object
        .get("abi")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} has no abi field"))?;
    if abi != "inbharat-harness" {
        return Err(format!("{name} names the wrong ABI: {abi}"));
    }
    let symbols = object
        .get("symbols")
        .ok_or_else(|| format!("{name} lists no symbols"))?;
    string_array(symbols, &format!("{name} symbols"))
}

fn all_exports() -> Result<Vec<String>, String> {
    let mut symbols = manifest_symbols(ABI_V1, "abi-v1.json")?;
    symbols.extend(manifest_symbols(ABI_V2, "abi-v2.json")?);
    Ok(symbols)
}

#[test]
fn every_manifest_symbol_is_an_extern_export_in_the_crate() -> Result<(), String> {
    for symbol in all_exports()? {
        let in_source = [SOURCE_LIB, SOURCE_V2]
            .iter()
            .any(|source| source.contains(&format!("fn {symbol}(")));
        if !in_source {
            return Err(format!(
                "{symbol} is declared in a manifest but not defined in this crate"
            ));
        }
    }
    Ok(())
}

#[test]
fn every_manifest_symbol_is_declared_in_the_c_header() -> Result<(), String> {
    for symbol in all_exports()? {
        if !HEADER.contains(&symbol) {
            return Err(format!(
                "{symbol} is exported by the crate but missing from include/inbharat_harness.h"
            ));
        }
    }
    Ok(())
}

#[test]
fn every_header_function_is_in_a_manifest() -> Result<(), String> {
    // Header function declarations, e.g. `int32_t ib_harness_run_v2(...)`.
    // The callback typedefs share the return-type shape and are skipped; so
    // is the v1 `const char *` status message.
    let mut declared = Vec::new();
    for line in HEADER.lines() {
        let trimmed = line.trim();
        let candidate = trimmed
            .strip_prefix("int32_t ")
            .or_else(|| trimmed.strip_prefix("uint32_t "))
            .unwrap_or_default();
        let name = candidate.split('(').next().unwrap_or_default().trim();
        if name.starts_with("ib_harness_") {
            declared.push(name);
        }
    }
    // 11 v1 functions (10 int32_t/uint32_t + the const-char status message)
    // + 10 v2.
    if declared.len() < 20 {
        return Err(format!(
            "the header scan found only {} declarations, expected at least 20",
            declared.len()
        ));
    }
    let exports = all_exports()?;
    for name in declared {
        if !exports.iter().any(|export| export == name) {
            return Err(format!(
                "{name} is declared in the header but missing from both manifests"
            ));
        }
    }
    Ok(())
}

#[test]
fn api_version_calls_match_the_manifest_majors() {
    assert_eq!(inbharat_harness::ib_harness_api_version_v1(), 1);
    assert_eq!(v2::ib_harness_api_version_v2(), 2);
}

#[test]
fn v2_manifest_documents_the_out_span_and_rejected_registration_contract() {
    assert!(
        ABI_V2.contains("next invocation or destroy"),
        "the out-span ownership contract is documented"
    );
    assert!(
        ABI_V2.contains("REJECTED registration consumes the builder"),
        "the rejected-registration lifecycle is documented"
    );
    assert!(
        Value::parse_json(ABI_V2).is_ok(),
        "abi-v2.json is valid bounded JSON"
    );
    assert!(
        ABI_V2.contains("\"compatibility\""),
        "v2 states its v1 compatibility"
    );
}
