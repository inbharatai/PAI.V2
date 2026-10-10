//! Bound nesting and reject duplicate keys before serde builds a Value (including escaped keys).
use crate::{require, Result};
use std::collections::BTreeSet;
pub fn preflight(input: &[u8], max: usize) -> Result<()> {
    require(input.len() <= max, "JSON byte bound")?;
    std::str::from_utf8(input).map_err(|_| "Invalid UTF-8")?;
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'{' => {
                stack.push(Some(BTreeSet::new()));
                require(stack.len() <= 16, "JSON depth")?;
            }
            b'[' => {
                stack.push(None);
                require(stack.len() <= 16, "JSON depth")?;
            }
            b'}' | b']' => {
                require(stack.pop().is_some(), "JSON nesting")?;
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut escaped = false;
                while i < input.len() {
                    let c = input[i];
                    if !escaped && c == b'"' {
                        break;
                    }
                    escaped = !escaped && c == b'\\';
                    i += 1;
                }
                require(i < input.len(), "JSON string truncated")?;
                let mut next = i + 1;
                while next < input.len() && input[next].is_ascii_whitespace() {
                    next += 1;
                }
                if input.get(next) == Some(&b':') {
                    let key: String =
                        serde_json::from_slice(&input[start..=i]).map_err(|_| "JSON key")?;
                    require(
                        stack
                            .last_mut()
                            .and_then(Option::as_mut)
                            .is_some_and(|keys| keys.insert(key)),
                        "Duplicate/invalid JSON key",
                    )?;
                }
            }
            _ => (),
        }
        i += 1;
    }
    require(stack.is_empty(), "JSON truncated")
}
