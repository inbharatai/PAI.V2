//! Actual production helper + unmodified actual Core MemoryQuery validation/runtime.
//! No Tauri execution, native model, vault provider, or provider-contract replica.
#![allow(dead_code)]
#[path = "../../src-tauri/src/chat_context.rs"]
mod production;
use inbharat_harness_core::{
    CancellationToken, EchoModelProvider, ExecutionLevel, HarnessBuilder, HarnessResult,
    MemoryCapabilities, MemoryOptions, MemoryProvider, MemoryQuery, MemoryRecord, MemoryScope,
    ModelChunk, ModelProvider, ModelRequest, ModelResponse, RunOptions,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
#[derive(Default)]
struct CheckedMemory {
    capabilities: AtomicUsize,
    attempted: AtomicUsize,
    valid_queries: AtomicUsize,
}
impl MemoryProvider for CheckedMemory {
    fn capabilities(&self) -> MemoryCapabilities {
        self.capabilities.fetch_add(1, Ordering::SeqCst);
        MemoryCapabilities {
            scopes: vec![
                MemoryScope::Preferences,
                MemoryScope::Relevant,
                MemoryScope::Project,
            ],
            can_retrieve: false,
            can_search: true,
            can_store: false,
            can_update: false,
            can_delete: false,
            max_results: 8,
        }
    }
    fn retrieve(&self, _: MemoryScope, _: &str, _: &str) -> HarnessResult<Option<MemoryRecord>> {
        panic!("unexpected retrieve")
    }
    fn search(&self, q: &MemoryQuery) -> HarnessResult<Vec<MemoryRecord>> {
        self.attempted.fetch_add(1, Ordering::SeqCst);
        q.validate()?; // Actual checked Core contract used by desktop adapter memory.rs:675.
        self.valid_queries.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
    fn store(&self, _: MemoryRecord) -> HarnessResult<()> {
        panic!("conversation writes must stay disabled")
    }
    fn update(&self, _: MemoryRecord) -> HarnessResult<()> {
        panic!("unexpected update")
    }
    fn delete(&self, _: MemoryScope, _: &str, _: &str) -> HarnessResult<bool> {
        panic!("unexpected delete")
    }
}
#[derive(Default)]
struct CheckedModel {
    calls: AtomicUsize,
    prompts: Mutex<Vec<String>>,
}
impl ModelProvider for CheckedModel {
    fn id(&self) -> &str {
        "echo"
    }
    fn models(&self) -> Vec<String> {
        vec!["echo-v1".into()]
    }
    fn stream(
        &self,
        request: &ModelRequest,
        cancel: &CancellationToken,
        sink: &mut dyn FnMut(ModelChunk) -> HarnessResult<()>,
    ) -> HarnessResult<ModelResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prompts
            .lock()
            .unwrap()
            .extend(request.messages.iter().map(|m| m.content.clone()));
        // Keep synthetic Echo output small; input was recorded verbatim above.
        // Otherwise Echo's long response hits an unrelated default output budget.
        let mut output_request = request.clone();
        for message in &mut output_request.messages {
            message.content = "bounded probe response".into();
        }
        EchoModelProvider::default().stream(&output_request, cancel, sink)
    }
}
fn accepts_query_bytes(bytes: usize) -> bool {
    MemoryQuery {
        scope: MemoryScope::Relevant,
        namespace: Some("vault-a".into()),
        text: "r".repeat(bytes),
        limit: 8,
    }
    .validate()
    .is_ok()
}
fn checked_core_query_limit() -> usize {
    // Discover threshold through actual Core public validation, not copied constant.
    let (mut accepted, mut rejected) = (0, 256 * 1024 + 1);
    assert!(accepts_query_bytes(accepted));
    assert!(!accepts_query_bytes(rejected));
    while accepted + 1 < rejected {
        let mid = (accepted + rejected) / 2;
        if accepts_query_bytes(mid) {
            accepted = mid;
        } else {
            rejected = mid;
        }
    }
    accepted
}
fn memory_options(cap: usize) -> MemoryOptions {
    // Structural options copy of current desktop bridge; NOT executing Tauri.
    MemoryOptions {
        scopes: if cap == 0 {
            vec![]
        } else {
            vec![
                MemoryScope::Preferences,
                MemoryScope::Relevant,
                MemoryScope::Project,
            ]
        },
        namespace: "vault-a".into(),
        conversation_namespace: Some("vault-a:conversation:hardening".into()),
        search_limit: 8,
        recent_conversation_limit: 16,
        max_context_bytes: cap,
        write_conversation: false,
    }
}
fn run(prompt: &str, cap: usize) -> (Arc<CheckedMemory>, Arc<CheckedModel>, bool) {
    let memory = Arc::new(CheckedMemory::default());
    let model = Arc::new(CheckedModel::default());
    let harness = HarnessBuilder::local_embedded(std::env::temp_dir())
        .unwrap()
        .register_model(model.clone())
        .unwrap()
        .memory_provider(memory.clone())
        .build();
    let options = RunOptions {
        provider: "echo".into(),
        model: "echo-v1".into(),
        explicit_level: Some(ExecutionLevel::L0),
        memory: memory_options(cap),
        ..RunOptions::default()
    };
    let result = harness.run(prompt, &options, &CancellationToken::new());
    eprintln!("prompt_bytes={} memory_cap={cap} capabilities={} attempted_queries={} valid_queries={} model_calls={} run_ok={} error={:?}", prompt.len(), memory.capabilities.load(Ordering::SeqCst), memory.attempted.load(Ordering::SeqCst), memory.valid_queries.load(Ordering::SeqCst), model.calls.load(Ordering::SeqCst), result.is_ok(), result.as_ref().err());
    (memory, model, result.is_ok())
}
#[test]
fn unchecked_retrieval_negative_control_fails_before_model_at_actual_core_limit() {
    let limit = checked_core_query_limit();
    eprintln!("actual Core MemoryQuery::validate discovered accepted text maximum={limit}");
    let (memory, model, ok) = run(&"r".repeat(limit + 1), 32768);
    assert!(!ok);
    assert_eq!(memory.attempted.load(Ordering::SeqCst), 1);
    assert_eq!(memory.valid_queries.load(Ordering::SeqCst), 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
}
#[test]
fn actual_core_contract_boundary_routes_retrieval_without_altering_current_request() {
    let limit = checked_core_query_limit();
    for bytes in [limit - 1, limit, limit + 1, 256 * 1024] {
        let request = "r".repeat(bytes);
        let result = production::assemble_prompt(
            &request,
            &[],
            false,
            production::ContextLimits::from_granted_context(None),
        );
        assert_eq!(result.prompt.as_bytes(), request.as_bytes());
        let (memory, model, ok) = run(&result.prompt, result.memory_max_context_bytes);
        assert!(
            ok,
            "accepted desktop request of {bytes} bytes aborted at memory boundary"
        );
        assert_eq!(model.calls.load(Ordering::SeqCst), 1);
        assert!(model
            .prompts
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.as_bytes() == request.as_bytes()));
        let queries = if bytes <= limit { 3 } else { 0 };
        assert_eq!(memory.attempted.load(Ordering::SeqCst), queries);
        assert_eq!(memory.valid_queries.load(Ordering::SeqCst), queries);
        if bytes > limit {
            assert_eq!(memory.capabilities.load(Ordering::SeqCst), 0);
            assert_eq!(result.memory_max_context_bytes, 0);
            assert!(result
                .context_note
                .as_deref()
                .unwrap()
                .contains(&format!("{limit}-byte memory query limit")));
        }
    }
}
#[test]
fn actual_core_guard_covers_custom_assembled_prompt_and_zero_cap_with_nonempty_scopes() {
    let limit = checked_core_query_limit();
    let request = "r".repeat(limit - 100);
    let user = "u".repeat(100);
    let assistant = "a".repeat(100);
    let history = [
        production::HistoryEntry {
            role: "user",
            text: Some(&user),
        },
        production::HistoryEntry {
            role: "assistant",
            text: Some(&assistant),
        },
    ];
    let result = production::assemble_prompt(
        &request,
        &history,
        false,
        production::ContextLimits {
            prompt_byte_budget: limit + 4096,
            max_entries: 24,
            per_entry_bytes: 8192,
        },
    );
    assert!(result.prompt.len() > limit);
    assert_eq!(result.memory_max_context_bytes, 0);
    let (memory, model, ok) = run(&result.prompt, result.memory_max_context_bytes);
    assert!(ok);
    assert_eq!(memory.attempted.load(Ordering::SeqCst), 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    let memory = Arc::new(CheckedMemory::default());
    let harness = HarnessBuilder::local_embedded(std::env::temp_dir())
        .unwrap()
        .register_model(Arc::new(CheckedModel::default()))
        .unwrap()
        .memory_provider(memory.clone())
        .build();
    let mut options = memory_options(32768);
    options.max_context_bytes = 0;
    harness
        .run(
            &result.prompt,
            &RunOptions {
                provider: "echo".into(),
                model: "echo-v1".into(),
                explicit_level: Some(ExecutionLevel::L0),
                memory: options,
                ..RunOptions::default()
            },
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(memory.capabilities.load(Ordering::SeqCst), 0);
}
fn supplied_fixture(text: &str) -> Vec<production::HistoryEntry<'_>> {
    text.lines()
        .map(|line| {
            let (role, text) = line.split_once('\t').unwrap();
            production::HistoryEntry {
                role,
                text: Some(text),
            }
        })
        .collect()
}
fn roles(prompt: &str) -> Vec<&str> {
    prompt
        .lines()
        .filter_map(|line| {
            if line.starts_with("USER: ") {
                Some("user")
            } else if line.starts_with("ASSISTANT: ") {
                Some("assistant")
            } else {
                None
            }
        })
        .collect()
}
#[test]
fn actual_frontend_selected_pairs_remain_complete_under_byte_and_entry_pressure() {
    let fixture_root = std::path::PathBuf::from(
        std::env::var("CORE_TEST_FIXTURES").expect("run via test:context:core"),
    );
    for filename in [
        "one-pair.tsv",
        "utf8-pair.tsv",
        "ten-pairs.tsv",
        "default-edge.tsv",
    ] {
        let fixture = std::fs::read_to_string(fixture_root.join(filename)).unwrap();
        let history = supplied_fixture(&fixture);
        assert!(history.len() >= 2 && history.len() % 2 == 0);
        for max_entries in [0, 1, 2, 3, 9, 19, 24] {
            for budget in [0, 80, 95, 100, 128, 512, 1024, 4096, 48 * 1024] {
                let result = production::assemble_prompt(
                    "Make it shorter",
                    &history,
                    true,
                    production::ContextLimits {
                        prompt_byte_budget: budget,
                        max_entries,
                        per_entry_bytes: 8192,
                    },
                );
                let retained = roles(&result.prompt);
                assert_eq!(
                    retained.len() % 2,
                    0,
                    "cap={budget}, max_entries={max_entries}, roles={retained:?}"
                );
                assert!(retained.chunks(2).all(|pair| pair == ["user", "assistant"]));
                assert!(retained.len() <= max_entries);
                let lines: Vec<_> = result
                    .prompt
                    .lines()
                    .filter_map(|line| {
                        line.strip_prefix("USER: ")
                            .or_else(|| line.strip_prefix("ASSISTANT: "))
                    })
                    .collect();
                for (bounded, supplied) in lines.iter().zip(&history[history.len() - lines.len()..])
                {
                    let prefix = bounded
                        .strip_suffix(" [history turn truncated]")
                        .unwrap_or(bounded);
                    assert!(!prefix.is_empty());
                    assert!(
                        supplied.text.unwrap().starts_with(prefix),
                        "must retain newest chronological supplied units, not substitute old ones"
                    );
                }
                assert!(result.prompt.len() <= budget.max("Make it shorter".len()));
                assert!(result.prompt.ends_with("Make it shorter"));
            }
        }
    }
}
