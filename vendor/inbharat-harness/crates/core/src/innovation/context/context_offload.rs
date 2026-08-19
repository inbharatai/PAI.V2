//! Context offload: Keep tokens out of context using sub-agents, skills, files, and background tasks.
//!
//! Based on Writer Agent Harness patterns: sub-agents as context firewalls, progressive disclosure,
//! tool output spillover, and background task delivery.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Context offload manager
pub struct ContextOffload {
    sub_agents: Arc<Mutex<HashMap<String, SubAgent>>>,
    skills: Arc<Mutex<HashMap<String, Skill>>>,
    file_spills: Arc<Mutex<HashMap<String, FileSpill>>>,
    background_tasks: Arc<Mutex<HashMap<String, BackgroundTask>>>,
    max_context_tokens: usize,
    current_context_tokens: usize,
}

#[derive(Clone, Debug)]
pub struct SubAgent {
    pub agent_id: String,
    pub task: String,
    pub status: SubAgentStatus,
    pub result_summary: String,
    pub citations: Vec<String>,
    pub token_cost: usize,
    pub max_summary_tokens: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SubAgentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub full_content_path: PathBuf,
    pub seed_content: String,
    pub max_seed_tokens: usize,
    pub loaded: bool,
}

#[derive(Clone, Debug)]
pub struct FileSpill {
    pub spill_id: String,
    pub file_path: PathBuf,
    pub preview: String,
    pub full_size: usize,
    pub preview_tokens: usize,
    pub content_type: String,
}

#[derive(Clone, Debug)]
pub struct BackgroundTask {
    pub task_id: String,
    pub task_type: String,
    pub status: BackgroundTaskStatus,
    pub result: Option<String>,
    pub progress: f64,
    pub started_at: u64,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl ContextOffload {
    pub fn new(max_context_tokens: usize) -> Self {
        Self {
            sub_agents: Arc::new(Mutex::new(HashMap::new())),
            skills: Arc::new(Mutex::new(HashMap::new())),
            file_spills: Arc::new(Mutex::new(HashMap::new())),
            background_tasks: Arc::new(Mutex::new(HashMap::new())),
            max_context_tokens,
            current_context_tokens: 0,
        }
    }

    /// Spawn a sub-agent for a task
    pub fn spawn_sub_agent(&self, task: String, max_summary_tokens: usize) -> String {
        let agent_id = format!("subagent-{}", crate::innovation::short_id());

        let sub_agent = SubAgent {
            agent_id: agent_id.clone(),
            task,
            status: SubAgentStatus::Running,
            result_summary: String::new(),
            citations: vec![],
            token_cost: 0,
            max_summary_tokens,
        };

        if let Ok(mut agents) = self.sub_agents.lock() {
            agents.insert(agent_id.clone(), sub_agent);
        }
        agent_id
    }

    /// Complete a sub-agent task
    pub fn complete_sub_agent(
        &self,
        agent_id: &str,
        result_summary: String,
        citations: Vec<String>,
        token_cost: usize,
    ) {
        if let Ok(mut agents) = self.sub_agents.lock() {
            if let Some(agent) = agents.get_mut(agent_id) {
                agent.status = SubAgentStatus::Completed;
                agent.result_summary = result_summary;
                agent.citations = citations;
                agent.token_cost = token_cost;
            }
        }
    }

    /// Get sub-agent result summary
    pub fn get_sub_agent_result(&self, agent_id: &str) -> Option<SubAgent> {
        self.sub_agents
            .lock()
            .map_or(None, |agents| agents.get(agent_id).cloned())
    }

    /// Register a skill with progressive disclosure
    pub fn register_skill(
        &self,
        skill_id: String,
        name: String,
        description: String,
        full_content_path: PathBuf,
        seed_content: String,
        max_seed_tokens: usize,
    ) {
        let skill = Skill {
            skill_id: skill_id.clone(),
            name,
            description,
            full_content_path,
            seed_content,
            max_seed_tokens,
            loaded: false,
        };

        if let Ok(mut skills) = self.skills.lock() {
            skills.insert(skill_id, skill);
        }
    }

    /// Load a skill when needed
    pub fn load_skill(&self, skill_id: &str) -> Option<Skill> {
        if let Ok(mut skills) = self.skills.lock() {
            if let Some(skill) = skills.get_mut(skill_id) {
                if !skill.loaded {
                    // Load full content, bounded to max_seed_tokens.
                    if let Ok(content) = std::fs::read_to_string(&skill.full_content_path) {
                        skill.seed_content =
                            Self::truncate_to_token_budget(&content, skill.max_seed_tokens);
                        skill.loaded = true;
                    }
                }
                return Some(skill.clone());
            }
        }
        None
    }

    /// Truncate `content` to at most `max_tokens` tokens.
    ///
    /// Token counting is a proxy: `chars / 4` (a common rough heuristic), not a
    /// real tokenizer. Truncation is char-boundary safe and appends an ellipsis
    /// marker when content was cut, so the caller can tell it was bounded.
    fn truncate_to_token_budget(content: &str, max_tokens: usize) -> String {
        let max_chars = max_tokens.saturating_mul(4);
        if content.len() <= max_chars {
            return content.to_string();
        }
        // Find a valid UTF-8 char boundary at or before max_chars.
        let mut end = max_chars;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &content[..end])
    }

    /// Get skill seed content (for context)
    pub fn get_skill_seed(&self, skill_id: &str) -> Option<String> {
        self.skills.lock().map_or(None, |skills| {
            skills.get(skill_id).map(|s| s.seed_content.clone())
        })
    }

    /// Spill large content to a file
    pub fn spill_to_file(
        &self,
        content: String,
        content_type: String,
        preview_tokens: usize,
    ) -> String {
        let spill_id = format!("spill-{}", crate::innovation::short_id());
        let file_path = PathBuf::from(format!("/tmp/{}", spill_id));

        // Write full content to file
        if std::fs::write(&file_path, &content).is_err() {
            return String::new();
        }

        // Create preview, bounded to preview_tokens (chars/4 proxy) and
        // char-boundary safe so multi-byte UTF-8 never panics the slice.
        let preview = Self::truncate_to_token_budget(&content, preview_tokens);

        let spill = FileSpill {
            spill_id: spill_id.clone(),
            file_path,
            preview,
            full_size: content.len(),
            preview_tokens,
            content_type,
        };

        if let Ok(mut spills) = self.file_spills.lock() {
            spills.insert(spill_id.clone(), spill);
        }
        spill_id
    }

    /// Get file spill preview
    pub fn get_file_spill(&self, spill_id: &str) -> Option<FileSpill> {
        self.file_spills
            .lock()
            .map_or(None, |spills| spills.get(spill_id).cloned())
    }

    /// Start a background task
    pub fn start_background_task(&self, task_type: String) -> String {
        let task_id = format!("task-{}", crate::innovation::short_id());

        let task = BackgroundTask {
            task_id: task_id.clone(),
            task_type,
            status: BackgroundTaskStatus::Running,
            result: None,
            progress: 0.0,
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            completed_at: None,
        };

        if let Ok(mut tasks) = self.background_tasks.lock() {
            tasks.insert(task_id.clone(), task);
        }
        task_id
    }

    /// Complete a background task
    pub fn complete_background_task(&self, task_id: &str, result: String) {
        if let Ok(mut tasks) = self.background_tasks.lock() {
            if let Some(task) = tasks.get_mut(task_id) {
                task.status = BackgroundTaskStatus::Completed;
                task.result = Some(result);
                task.progress = 1.0;
                task.completed_at = Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                );
            }
        }
    }

    /// Get background task status
    pub fn get_background_task(&self, task_id: &str) -> Option<BackgroundTask> {
        self.background_tasks
            .lock()
            .map_or(None, |tasks| tasks.get(task_id).cloned())
    }

    /// Check if context needs offloading
    pub fn needs_offload(&self, current_tokens: usize) -> bool {
        current_tokens > self.max_context_tokens * 80 / 100
    }

    /// Offload context to stay within budget
    pub fn offload_context(&self, current_tokens: usize) -> OffloadResult {
        if !self.needs_offload(current_tokens) {
            return OffloadResult::NoOffloadNeeded;
        }

        let excess = current_tokens - (self.max_context_tokens * 80 / 100);
        let mut freed = 0;

        // Offload oldest sub-agent results
        if let Ok(mut agents) = self.sub_agents.lock() {
            let mut to_remove = Vec::new();

            for (id, agent) in agents.iter() {
                if agent.status == SubAgentStatus::Completed && agent.token_cost > 0 {
                    to_remove.push(id.clone());
                    freed += agent.token_cost;
                    if freed >= excess {
                        break;
                    }
                }
            }

            for id in to_remove {
                agents.remove(&id);
            }
        }

        // Offload old file spills
        if let Ok(mut spills) = self.file_spills.lock() {
            let mut to_remove = Vec::new();

            for (id, spill) in spills.iter() {
                to_remove.push(id.clone());
                freed += spill.preview_tokens;
                if freed >= excess {
                    break;
                }
            }

            for id in to_remove {
                spills.remove(&id);
            }
        }

        OffloadResult::Offloaded {
            tokens_freed: freed,
        }
    }

    /// Get current context usage
    pub fn get_context_usage(&self) -> ContextUsage {
        let sub_agent_tokens: usize = self
            .sub_agents
            .lock()
            .map_or(0, |agents| agents.values().map(|a| a.token_cost).sum());
        let skill_tokens: usize = self.skills.lock().map_or(0, |skills| {
            skills.values().map(|s| s.seed_content.len() / 4).sum()
        });
        let spill_tokens: usize = self
            .file_spills
            .lock()
            .map_or(0, |spills| spills.values().map(|s| s.preview_tokens).sum());
        let task_tokens: usize = self.background_tasks.lock().map_or(0, |tasks| {
            tasks
                .values()
                .map(|t| t.result.as_ref().map_or(0, |r| r.len() / 4))
                .sum()
        });

        ContextUsage {
            total_tokens: self.current_context_tokens,
            max_tokens: self.max_context_tokens,
            utilization: self.current_context_tokens as f64 / self.max_context_tokens as f64,
            sub_agent_tokens,
            skill_tokens,
            spill_tokens,
            task_tokens,
        }
    }
}

#[derive(Clone, Debug)]
pub enum OffloadResult {
    NoOffloadNeeded,
    Offloaded { tokens_freed: usize },
}

#[derive(Clone, Debug)]
pub struct ContextUsage {
    pub total_tokens: usize,
    pub max_tokens: usize,
    pub utilization: f64,
    pub sub_agent_tokens: usize,
    pub skill_tokens: usize,
    pub spill_tokens: usize,
    pub task_tokens: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_offload() {
        let offload = ContextOffload::new(1000);

        // Spawn sub-agent
        let agent_id = offload.spawn_sub_agent("Test task".to_string(), 100);
        assert!(!agent_id.is_empty());

        // Complete sub-agent
        offload.complete_sub_agent(
            &agent_id,
            "Test result".to_string(),
            vec!["citation1".to_string()],
            50,
        );

        let result = offload.get_sub_agent_result(&agent_id);
        assert!(result.is_some());
        if let Some(result) = result {
            assert_eq!(result.result_summary, "Test result");
        }

        // Register skill (write the backing file so load_skill can read it)
        let skill_path = std::env::temp_dir().join("inbharat_test_skill.txt");
        assert!(std::fs::write(&skill_path, "Full skill content").is_ok());
        offload.register_skill(
            "skill1".to_string(),
            "Test Skill".to_string(),
            "A test skill".to_string(),
            skill_path.clone(),
            "Test seed content".to_string(),
            100,
        );

        // Load skill
        let skill = offload.load_skill("skill1");
        assert!(skill.is_some());
        if let Some(skill) = skill {
            assert!(skill.loaded);
        }
        let _ = std::fs::remove_file(&skill_path);

        // Spill to file
        let spill_id = offload.spill_to_file(
            "Large content that exceeds context limits".to_string(),
            "text".to_string(),
            10,
        );
        assert!(!spill_id.is_empty());

        let spill = offload.get_file_spill(&spill_id);
        assert!(spill.is_some());
        if let Some(spill) = spill {
            assert!(spill.preview.contains("..."));
        }

        // Start background task
        let task_id = offload.start_background_task("test_task".to_string());
        assert!(!task_id.is_empty());

        // Complete background task
        offload.complete_background_task(&task_id, "Task result".to_string());

        let task = offload.get_background_task(&task_id);
        assert!(task.is_some());
        if let Some(task) = task {
            assert_eq!(task.status, BackgroundTaskStatus::Completed);
        }

        // Test context offload
        let result = offload.offload_context(900);
        assert!(matches!(result, OffloadResult::Offloaded { .. }));

        // Test context usage
        let usage = offload.get_context_usage();
        assert!(usage.utilization >= 0.0);
    }

    #[test]
    fn test_load_skill_enforces_max_seed_tokens() {
        let offload = ContextOffload::new(1000);

        // 400 'a's = 100 tokens under the chars/4 proxy; cap at 10 tokens (40 chars).
        let big_content = "a".repeat(400);
        let skill_path = std::env::temp_dir().join("inbharat_test_skill_truncate.txt");
        assert!(std::fs::write(&skill_path, &big_content).is_ok());

        offload.register_skill(
            "skill-trunc".to_string(),
            "Trunc Skill".to_string(),
            "seed cap test".to_string(),
            skill_path.clone(),
            "seed".to_string(),
            10, // max_seed_tokens = 10 -> 40 chars budget
        );

        let skill = offload.load_skill("skill-trunc");
        assert!(skill.is_some());
        if let Some(skill) = skill {
            assert!(skill.loaded);
            // Old code stored the full 400 chars; new code truncates to ~40 + "...".
            assert!(
                skill.seed_content.len() <= 40 + 3,
                "seed_content was not bounded to max_seed_tokens: len={}",
                skill.seed_content.len()
            );
            assert!(skill.seed_content.ends_with("..."));
        }

        // Also verify through the public seed accessor.
        let seed = offload.get_skill_seed("skill-trunc");
        assert!(seed.is_some());
        if let Some(seed) = seed {
            assert!(seed.len() <= 43);
        }

        let _ = std::fs::remove_file(&skill_path);
    }

    #[test]
    fn test_load_skill_short_content_not_truncated() {
        let offload = ContextOffload::new(1000);

        let skill_path = std::env::temp_dir().join("inbharat_test_skill_short.txt");
        assert!(std::fs::write(&skill_path, "short content").is_ok());

        offload.register_skill(
            "skill-short".to_string(),
            "Short Skill".to_string(),
            "no truncation needed".to_string(),
            skill_path.clone(),
            "seed".to_string(),
            100, // 100 tokens -> 400 chars budget; content is far under
        );

        let skill = offload.load_skill("skill-short");
        assert!(skill.is_some());
        if let Some(skill) = skill {
            assert_eq!(skill.seed_content, "short content");
        }

        let _ = std::fs::remove_file(&skill_path);
    }

    #[test]
    fn test_spill_preview_is_char_boundary_safe() {
        let offload = ContextOffload::new(1000);

        // Multi-byte chars: each 'é' is 2 bytes. preview_tokens=1 -> 4-byte budget,
        // which lands mid-codepoint on old code (panic). New code backs off to a boundary.
        let content = "é".repeat(10); // 20 bytes
        let spill_id = offload.spill_to_file(content, "text".to_string(), 1);
        assert!(!spill_id.is_empty());

        let spill = offload.get_file_spill(&spill_id);
        assert!(spill.is_some());
        if let Some(spill) = spill {
            // 4-byte budget can hold at most 2 'é' (4 bytes) + "..." marker.
            assert!(spill.preview.len() <= 4 + 3);
        }
    }
}
