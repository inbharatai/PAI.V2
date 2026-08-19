use inbharat_harness_core::error::HarnessResult;
use inbharat_harness_core::routing::{ExecutionLevel, RoutePolicy, RouteRequest, Router};

#[test]
fn six_hundred_ordinary_prompts_never_activate_agent() -> HarnessResult<()> {
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
    let router = Router;
    let mut tested = 0_usize;
    for subject in SUBJECTS {
        for template in TEMPLATES {
            let prompt = template.replace("{}", subject);
            let decision = router.route(&RouteRequest::new(&prompt), RoutePolicy::default())?;
            assert_eq!(decision.level, ExecutionLevel::L0, "prompt={prompt}");
            tested += 1;
        }
    }
    assert_eq!(tested, 600);
    Ok(())
}

#[test]
fn agent_words_inside_ordinary_sentences_do_not_trigger() -> HarnessResult<()> {
    let router = Router;
    for prompt in [
        "What does a travel agent do?",
        "Explain software agents in one paragraph.",
        "I read file formats for fun.",
        "A command can be a sentence in grammar.",
        "The build process is interesting.",
        "Tell me what a workspace means.",
        "Build a complete understanding of gravity.",
        "Implement a complete explanation of photosynthesis.",
        "Create a complete website outline for discussion.",
        "Plan a complete application without building it.",
        "Explain how to build a complete website.",
    ] {
        let decision = router.route(&RouteRequest::new(prompt), RoutePolicy::default())?;
        assert_eq!(decision.level, ExecutionLevel::L0);
    }
    Ok(())
}
