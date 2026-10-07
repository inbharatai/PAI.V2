//! Thin executable probe of the sibling production helper, not a classifier replica.
#![allow(dead_code)]
#[path = "../../src-tauri/src/chat_context.rs"]
mod production;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let message = args.get(1).expect("typed message required");
    let has_attachments = args.get(2).map(|value| value == "true").unwrap_or(false);
    let history = [
        production::HistoryEntry {
            role: "user",
            text: Some("Create Small Giant knowledge-base plan"),
        },
        production::HistoryEntry {
            role: "assistant",
            text: Some("PARITY_OLD_PLAN_SENTINEL"),
        },
    ];
    let result = production::assemble_prompt(
        message,
        &history,
        has_attachments,
        production::ContextLimits::from_granted_context(None),
    );
    println!(
        "{},{},{}",
        result.memory_max_context_bytes == 0,
        result.memory_max_context_bytes,
        result.prompt.contains("PARITY_OLD_PLAN_SENTINEL")
    );
}
