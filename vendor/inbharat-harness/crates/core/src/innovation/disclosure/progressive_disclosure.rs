//! Progressive disclosure: Skills and tools loaded only when needed.
//!
//! Skills use progressive disclosure (name+description only), tool catalogs degrade by model tier.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Skill descriptor
#[derive(Clone, Debug)]
pub struct SkillDescriptor {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub complexity: SkillComplexity,
    pub required_capabilities: Vec<String>,
    pub full_content: Option<String>,
    pub loaded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillComplexity {
    Simple,   // Simple skills
    Moderate, // Moderate skills
    Complex,  // Complex skills
    Expert,   // Expert-level skills
}

/// Tool descriptor
#[derive(Clone, Debug)]
pub struct ToolDescriptor {
    pub tool_id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub complexity: ToolComplexity,
    pub required_capabilities: Vec<String>,
    pub schema: Option<String>,
    pub loaded: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolComplexity {
    Basic,        // Basic tools
    Intermediate, // Intermediate tools
    Advanced,     // Advanced tools
    Expert,       // Expert-level tools
}

/// Progressive disclosure manager
pub struct ProgressiveDisclosure {
    skills: Arc<Mutex<HashMap<String, SkillDescriptor>>>,
    tools: Arc<Mutex<HashMap<String, ToolDescriptor>>>,
    model_tier: ModelTier,
    max_loaded_skills: usize,
    max_loaded_tools: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelTier {
    Basic,    // Basic models
    Standard, // Standard models
    Advanced, // Advanced models
    Expert,   // Expert models
}

impl ProgressiveDisclosure {
    pub fn new(model_tier: ModelTier) -> Self {
        Self {
            skills: Arc::new(Mutex::new(HashMap::new())),
            tools: Arc::new(Mutex::new(HashMap::new())),
            model_tier,
            max_loaded_skills: 10,
            max_loaded_tools: 20,
        }
    }

    /// Register a skill with progressive disclosure
    pub fn register_skill(&self, skill: SkillDescriptor) {
        if let Ok(mut skills) = self.skills.lock() {
            skills.insert(skill.skill_id.clone(), skill);
        }
    }

    /// Register a tool with progressive disclosure
    pub fn register_tool(&self, tool: ToolDescriptor) {
        if let Ok(mut tools) = self.tools.lock() {
            tools.insert(tool.tool_id.clone(), tool);
        }
    }

    /// Load a skill when needed
    pub fn load_skill(&self, skill_id: &str) -> Option<SkillDescriptor> {
        let mut skills = self.skills.lock().ok()?;

        // Enforce the loaded-skill budget (counted before the mutable borrow)
        let loaded_count = skills.values().filter(|s| s.loaded).count();

        let skill = skills.get_mut(skill_id)?;

        if !skill.loaded {
            // Check if model tier supports this skill
            if !self.can_load_skill(skill) || loaded_count >= self.max_loaded_skills {
                return None;
            }

            // Load full content
            skill.loaded = true;
        }

        Some(skill.clone())
    }

    /// Load a tool when needed
    pub fn load_tool(&self, tool_id: &str) -> Option<ToolDescriptor> {
        let mut tools = self.tools.lock().ok()?;

        // Enforce the loaded-tool budget (counted before the mutable borrow)
        let loaded_count = tools.values().filter(|t| t.loaded).count();

        let tool = tools.get_mut(tool_id)?;

        if !tool.loaded {
            // Check if model tier supports this tool
            if !self.can_load_tool(tool) || loaded_count >= self.max_loaded_tools {
                return None;
            }

            // Load full schema
            tool.loaded = true;
        }

        Some(tool.clone())
    }

    /// Get available skills for current model tier
    pub fn get_available_skills(&self) -> Vec<SkillDescriptor> {
        self.skills.lock().map_or_else(
            |_| Vec::new(),
            |skills| {
                skills
                    .values()
                    .filter(|s| self.can_load_skill(s))
                    .cloned()
                    .collect()
            },
        )
    }

    /// Get available tools for current model tier
    pub fn get_available_tools(&self) -> Vec<ToolDescriptor> {
        self.tools.lock().map_or_else(
            |_| Vec::new(),
            |tools| {
                tools
                    .values()
                    .filter(|t| self.can_load_tool(t))
                    .cloned()
                    .collect()
            },
        )
    }

    /// Check if a skill can be loaded for current model tier
    fn can_load_skill(&self, skill: &SkillDescriptor) -> bool {
        // Check complexity requirements
        match (self.model_tier.clone(), skill.complexity.clone()) {
            (ModelTier::Basic, SkillComplexity::Simple) => true,
            (ModelTier::Basic, SkillComplexity::Moderate) => false,
            (ModelTier::Basic, SkillComplexity::Complex) => false,
            (ModelTier::Basic, SkillComplexity::Expert) => false,

            (ModelTier::Standard, SkillComplexity::Simple) => true,
            (ModelTier::Standard, SkillComplexity::Moderate) => true,
            (ModelTier::Standard, SkillComplexity::Complex) => false,
            (ModelTier::Standard, SkillComplexity::Expert) => false,

            (ModelTier::Advanced, SkillComplexity::Simple) => true,
            (ModelTier::Advanced, SkillComplexity::Moderate) => true,
            (ModelTier::Advanced, SkillComplexity::Complex) => true,
            (ModelTier::Advanced, SkillComplexity::Expert) => false,

            (ModelTier::Expert, _) => true,
        }
    }

    /// Check if a tool can be loaded for current model tier
    fn can_load_tool(&self, tool: &ToolDescriptor) -> bool {
        // Check complexity requirements
        match (self.model_tier.clone(), tool.complexity.clone()) {
            (ModelTier::Basic, ToolComplexity::Basic) => true,
            (ModelTier::Basic, ToolComplexity::Intermediate) => false,
            (ModelTier::Basic, ToolComplexity::Advanced) => false,
            (ModelTier::Basic, ToolComplexity::Expert) => false,

            (ModelTier::Standard, ToolComplexity::Basic) => true,
            (ModelTier::Standard, ToolComplexity::Intermediate) => true,
            (ModelTier::Standard, ToolComplexity::Advanced) => false,
            (ModelTier::Standard, ToolComplexity::Expert) => false,

            (ModelTier::Advanced, ToolComplexity::Basic) => true,
            (ModelTier::Advanced, ToolComplexity::Intermediate) => true,
            (ModelTier::Advanced, ToolComplexity::Advanced) => true,
            (ModelTier::Advanced, ToolComplexity::Expert) => false,

            (ModelTier::Expert, _) => true,
        }
    }

    /// Get skill seed content (for context)
    pub fn get_skill_seed(&self, skill_id: &str) -> Option<String> {
        self.skills
            .lock()
            .ok()
            .and_then(|skills| skills.get(skill_id).map(|s| s.description.clone()))
    }

    /// Get tool schema (for context)
    pub fn get_tool_schema(&self, tool_id: &str) -> Option<String> {
        self.tools
            .lock()
            .ok()
            .and_then(|tools| tools.get(tool_id).and_then(|t| t.schema.clone()))
    }

    /// Get loaded skills count
    pub fn get_loaded_skills_count(&self) -> usize {
        self.skills
            .lock()
            .map_or(0, |skills| skills.values().filter(|s| s.loaded).count())
    }

    /// Get loaded tools count
    pub fn get_loaded_tools_count(&self) -> usize {
        self.tools
            .lock()
            .map_or(0, |tools| tools.values().filter(|t| t.loaded).count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progressive_disclosure() {
        let disclosure = ProgressiveDisclosure::new(ModelTier::Standard);

        // Register skills
        disclosure.register_skill(SkillDescriptor {
            skill_id: "skill1".to_string(),
            name: "Simple Skill".to_string(),
            description: "A simple skill".to_string(),
            category: "general".to_string(),
            complexity: SkillComplexity::Simple,
            required_capabilities: vec![],
            full_content: None,
            loaded: false,
        });

        disclosure.register_skill(SkillDescriptor {
            skill_id: "skill2".to_string(),
            name: "Complex Skill".to_string(),
            description: "A complex skill".to_string(),
            category: "general".to_string(),
            complexity: SkillComplexity::Complex,
            required_capabilities: vec![],
            full_content: None,
            loaded: false,
        });

        // Register tools
        disclosure.register_tool(ToolDescriptor {
            tool_id: "tool1".to_string(),
            name: "Basic Tool".to_string(),
            description: "A basic tool".to_string(),
            category: "general".to_string(),
            complexity: ToolComplexity::Basic,
            required_capabilities: vec![],
            schema: None,
            loaded: false,
        });

        disclosure.register_tool(ToolDescriptor {
            tool_id: "tool2".to_string(),
            name: "Advanced Tool".to_string(),
            description: "An advanced tool".to_string(),
            category: "general".to_string(),
            complexity: ToolComplexity::Advanced,
            required_capabilities: vec![],
            schema: None,
            loaded: false,
        });

        // Load skills
        let skill1 = disclosure.load_skill("skill1");
        assert!(skill1.is_some());
        if let Some(skill1) = skill1 {
            assert!(skill1.loaded);
        }

        let skill2 = disclosure.load_skill("skill2");
        assert!(skill2.is_none()); // Complex skill not available for Standard tier

        // Load tools
        let tool1 = disclosure.load_tool("tool1");
        assert!(tool1.is_some());
        if let Some(tool1) = tool1 {
            assert!(tool1.loaded);
        }

        let tool2 = disclosure.load_tool("tool2");
        assert!(tool2.is_none()); // Advanced tool not available for Standard tier

        // Get available skills
        let available_skills = disclosure.get_available_skills();
        assert_eq!(available_skills.len(), 1); // Only simple skill available

        // Get available tools
        let available_tools = disclosure.get_available_tools();
        assert_eq!(available_tools.len(), 1); // Only basic tool available
    }
}
