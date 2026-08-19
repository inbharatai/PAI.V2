use inbharat_harness_core::Session;
use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use inbharat_harness_core::routing::{ExecutionLevel, RoutePolicy, RouteRequest, Router};
use std::fs;
use std::path::Path;
use std::time::Instant;

pub(crate) fn run(iterations: usize, output: Option<&Path>, version: &str) -> HarnessResult<()> {
    let iterations = iterations.clamp(1, 1_000_000);
    let router = Router;
    let policy = RoutePolicy::default();
    let ordinary = ordinary_prompts();
    let mut false_agent = 0_usize;
    for prompt in &ordinary {
        let decision = router.route(&RouteRequest::new(prompt), policy)?;
        if decision.level >= ExecutionLevel::L2 {
            false_agent = false_agent.saturating_add(1);
        }
    }
    let mixed = benchmark_prompts();
    let mut samples = Vec::with_capacity(iterations);
    let mut checksum = 0_u64;
    for index in 0..iterations {
        let prompt = mixed[index % mixed.len()];
        let started = Instant::now();
        let decision = router.route(&RouteRequest::new(prompt), policy)?;
        samples.push(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        checksum = checksum.wrapping_add(u64::from(decision.level as u8) + 1);
    }
    samples.sort_unstable();
    const SESSION_CHURN_COUNT: usize = 10_000;
    let session_started = Instant::now();
    let mut session_events_total = 0_usize;
    for _index in 0..SESSION_CHURN_COUNT {
        let session = Session::in_memory()?;
        session_events_total = session_events_total.saturating_add(session.events().len());
    }
    let session_churn_ns = u64::try_from(session_started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let json = format!(
        "{{\n  \"schema\": 1,\n  \"version\": \"{}\",\n  \"iterations\": {},\n  \"ordinary_prompts\": {},\n  \"false_agent_activations\": {},\n  \"false_agent_rate\": {:.8},\n  \"routing_ns_p50\": {},\n  \"routing_ns_p95\": {},\n  \"routing_ns_max\": {},\n  \"session_churn_count\": {},\n  \"session_churn_events\": {},\n  \"session_churn_ns_total\": {},\n  \"checksum\": {}\n}}\n",
        version,
        iterations,
        ordinary.len(),
        false_agent,
        false_agent as f64 / ordinary.len() as f64,
        percentile(&samples, 50),
        percentile(&samples, 95),
        samples.last().copied().unwrap_or(0),
        SESSION_CHURN_COUNT,
        session_events_total,
        session_churn_ns,
        checksum
    );
    print!("{json}");
    if let Some(path) = output {
        fs::write(path, &json).map_err(|error| {
            Failure::new(
                ErrorCode::ToolFailed,
                FailureClass::Execution,
                "benchmark.write",
                "cannot write benchmark output",
            )
            .with_detail("io_kind", format!("{:?}", error.kind()))
        })?;
    }
    if false_agent != 0 {
        return Err(Failure::new(
            ErrorCode::VerificationFailed,
            FailureClass::Policy,
            "benchmark.false_activation",
            "ordinary-prompt false agent activation gate failed",
        ));
    }
    Ok(())
}

pub(crate) fn ordinary_prompts() -> Vec<String> {
    const SUBJECTS: [&str; 20] = [
        "the weather",
        "history",
        "music",
        "gardening",
        "mathematics",
        "tea",
        "travel",
        "languages",
        "photography",
        "health",
        "books",
        "cooking",
        "sports",
        "design",
        "science",
        "education",
        "culture",
        "finance",
        "movies",
        "nature",
    ];
    const TEMPLATES: [&str; 30] = [
        "Tell me about {}.",
        "What is interesting about {}?",
        "Explain {} simply.",
        "Can you define {}?",
        "Give me a short overview of {}.",
        "Why do people enjoy {}?",
        "What are common questions about {}?",
        "Summarize the basics of {}.",
        "How would you teach {} to a beginner?",
        "Share three facts about {}.",
        "What vocabulary is useful for {}?",
        "Write a friendly sentence about {}.",
        "Is {} a broad topic?",
        "Compare two ordinary ideas in {}.",
        "What should a student know about {}?",
        "Help me understand {} without tools.",
        "Answer a general question concerning {}.",
        "Describe {} in plain language.",
        "Give a neutral explanation of {}.",
        "What is one misconception about {}?",
        "I am curious about {}.",
        "Could we discuss {}?",
        "Make a tiny quiz about {}.",
        "Offer a mnemonic for {}.",
        "What makes {} approachable?",
        "Name a beginner resource category for {}.",
        "How has {} changed over time?",
        "What is a balanced view of {}?",
        "State one question and answer about {}.",
        "Provide a concise response about {}.",
    ];
    let mut prompts = Vec::with_capacity(SUBJECTS.len() * TEMPLATES.len());
    for subject in SUBJECTS {
        for template in TEMPLATES {
            prompts.push(template.replace("{}", subject));
        }
    }
    prompts
}

fn benchmark_prompts() -> Vec<&'static str> {
    vec![
        "hello",
        "What is Rust?",
        "read file README.md",
        "list files",
        "research and compare local storage options",
        "analyze and summarize these constraints",
        "build a complete website in the workspace and test it",
        "/l2 inspect this bounded task",
        "/l3 implement a complete repository",
        "I read file formats for fun",
    ]
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let index = samples.len().saturating_sub(1).saturating_mul(percentile) / 100;
    samples[index]
}
