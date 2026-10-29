use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Represents a parsed agent skill with metadata, slash-command binding, and markdown instructions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    /// Unique identifier / name of the skill (e.g., "benchmark", "plan").
    pub name: String,
    /// Human-readable description of what this skill does and when to activate it.
    pub description: String,
    /// Slash command trigger (e.g., "/benchmark", "/plan").
    pub command: String,
    /// Key-value metadata extracted from YAML frontmatter (e.g. icon, category, model).
    pub metadata: HashMap<String, String>,
    /// Tags associated with the skill.
    pub tags: Vec<String>,
    /// Markdown documentation and detailed prompt instructions.
    pub content: String,
    /// Absolute filesystem path where the `SKILL.md` was discovered.
    pub path: PathBuf,
}

/// Registry managing dynamic skill discovery, slash-command mapping, and system prompt generation.
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    /// Internal map storing skills keyed by normalized skill name.
    skills: HashMap<String, Skill>,
    /// Map of command string (e.g. "/plan") to skill name.
    commands: HashMap<String, String>,
}

impl SkillRegistry {
    /// Creates an empty SkillRegistry.
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            commands: HashMap::new(),
        }
    }

    /// Registers or replaces a skill in the registry.
    pub fn register(&mut self, skill: Skill) {
        let normalized_cmd = if skill.command.starts_with('/') {
            skill.command.to_lowercase()
        } else {
            format!("/{}", skill.command.to_lowercase())
        };

        let key = skill.name.to_lowercase();
        self.commands.insert(normalized_cmd, key.clone());
        self.skills.insert(key, skill);
    }

    /// Removes a skill by name.
    pub fn remove(&mut self, name: &str) -> Option<Skill> {
        let key = name.to_lowercase();
        if let Some(skill) = self.skills.remove(&key) {
            let cmd = if skill.command.starts_with('/') {
                skill.command.to_lowercase()
            } else {
                format!("/{}", skill.command.to_lowercase())
            };
            self.commands.remove(&cmd);
            Some(skill)
        } else {
            None
        }
    }

    /// Retrieves a skill by name.
    pub fn get_skill(&self, name: &str) -> Option<&Skill> {
        self.skills.get(&name.to_lowercase())
    }

    /// Retrieves a skill by its slash-command (e.g. "/plan" or "plan").
    pub fn get_skill_by_command(&self, cmd: &str) -> Option<&Skill> {
        let normalized = if cmd.starts_with('/') {
            cmd.to_lowercase()
        } else {
            format!("/{}", cmd.to_lowercase())
        };

        let skill_name = self.commands.get(&normalized)?;
        self.skills.get(skill_name)
    }

    /// Returns a list of all registered skills sorted by name.
    pub fn list_skills(&self) -> Vec<&Skill> {
        let mut list: Vec<&Skill> = self.skills.values().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Returns the number of registered skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Returns true if the registry has no skills.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Recursively discovers and registers all `SKILL.md` files from workspace and user roots.
    pub async fn discover_skills(&mut self, workspace_dir: &Path) -> anyhow::Result<usize> {
        let mut discovery_roots: Vec<PathBuf> = Vec::new();

        // 1. Workspace-scoped skill directories
        let ws_agendex_skills = workspace_dir.join(".agendex").join("skills");
        if ws_agendex_skills.is_dir() {
            discovery_roots.push(ws_agendex_skills);
        }

        let ws_agenda_skills = workspace_dir.join(".agenda").join("skills");
        if ws_agenda_skills.is_dir() {
            discovery_roots.push(ws_agenda_skills);
        }

        let ws_skills = workspace_dir.join("skills");
        if ws_skills.is_dir() {
            discovery_roots.push(ws_skills);
        }

        let ws_agents_skills = workspace_dir.join(".agents").join("skills");
        if ws_agents_skills.is_dir() {
            discovery_roots.push(ws_agents_skills);
        }

        // 2. Global user-level skills directories
        if let Some(user_home) = Self::get_user_home() {
            let global_agendex_skills = user_home.join(".agendex").join("skills");
            if global_agendex_skills.is_dir() {
                discovery_roots.push(global_agendex_skills);
            }

            let global_agenda_skills = user_home.join(".agenda").join("skills");
            if global_agenda_skills.is_dir() {
                discovery_roots.push(global_agenda_skills);
            }
        }

        let mut discovered = 0;

        for root in discovery_roots {
            for entry in WalkDir::new(root)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path.is_file() {
                    let file_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();

                    if file_name == "skill.md" {
                        match Self::parse_skill_file(path) {
                            Ok(skill) => {
                                tracing::debug!(
                                    "Discovered skill: {} ({})",
                                    skill.name,
                                    skill.command
                                );
                                self.register(skill);
                                discovered += 1;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to parse skill at {}: {}",
                                    path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(discovered)
    }

    /// Reads and parses a `SKILL.md` file from disk.
    pub fn parse_skill_file(path: &Path) -> anyhow::Result<Skill> {
        let raw = fs::read_to_string(path)?;
        Self::parse_skill_content(&raw, path.to_path_buf())
    }

    /// Parses raw markdown text containing optional YAML frontmatter into a `Skill`.
    pub fn parse_skill_content(raw: &str, file_path: PathBuf) -> anyhow::Result<Skill> {
        // Strip BOM if present
        let clean_raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);

        let (frontmatter_map, body) = extract_frontmatter(clean_raw);

        // Fallback skill name from parent directory if not explicitly declared in frontmatter
        let default_name = file_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed_skill")
            .to_string();

        let name = frontmatter_map.get("name").cloned().unwrap_or(default_name);

        let description = frontmatter_map
            .get("description")
            .cloned()
            .unwrap_or_else(|| extract_first_paragraph(&body));

        let command = frontmatter_map
            .get("command")
            .cloned()
            .unwrap_or_else(|| format!("/{}", name));

        let tags: Vec<String> = frontmatter_map
            .get("tags")
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let mut metadata = HashMap::new();
        for (k, v) in frontmatter_map {
            if !["name", "description", "command", "tags"].contains(&k.as_str()) {
                metadata.insert(k, v);
            }
        }

        Ok(Skill {
            name,
            description,
            command,
            metadata,
            tags,
            content: body,
            path: file_path,
        })
    }

    /// Formats registered skills into slash command recommendations for system prompts.
    pub fn format_slash_commands_prompt(&self) -> String {
        let skills = self.list_skills();
        if skills.is_empty() {
            return String::new();
        }

        let mut out = String::from("Available slash commands you can recommend to the user:\n");
        for skill in skills {
            out.push_str(&format!("- {}: {}\n", skill.command, skill.description));
        }
        out
    }

    fn get_user_home() -> Option<PathBuf> {
        std::env::var("AGENDEX_HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .or_else(|_| std::env::var("HOME"))
            .ok()
            .map(PathBuf::from)
    }
}

/// Helper extracting YAML frontmatter block delimited by `---` and returning (key_values, markdown_body).
fn extract_frontmatter(raw: &str) -> (HashMap<String, String>, String) {
    let mut map = HashMap::new();
    let lines: Vec<&str> = raw.lines().collect();

    // Check if starts with frontmatter delimiter ---
    let mut start_idx = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "---" {
            start_idx = Some(i);
            break;
        }
        // First non-empty line isn't ---, so no frontmatter
        break;
    }

    let start_idx = match start_idx {
        Some(idx) => idx,
        None => return (map, raw.to_string()),
    };

    // Find closing delimiter ---
    let mut end_idx = None;
    for (i, line) in lines.iter().enumerate().skip(start_idx + 1) {
        if line.trim() == "---" {
            end_idx = Some(i);
            break;
        }
    }

    let end_idx = match end_idx {
        Some(idx) => idx,
        None => return (map, raw.to_string()),
    };

    // Parse frontmatter lines between start_idx and end_idx
    let mut i = start_idx + 1;
    while i < end_idx {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        if let Some(colon_pos) = trimmed.find(':') {
            let key = trimmed[..colon_pos].trim().to_string();
            let mut val_part = trimmed[colon_pos + 1..].trim();

            // Check for multi-line block scalar (>- or > or | or |-)
            if val_part == ">-" || val_part == ">" || val_part == "|" || val_part == "|-" {
                let is_folded = val_part.starts_with('>');
                let mut scalar_lines = Vec::new();
                i += 1;

                while i < end_idx {
                    let sub_line = lines[i];
                    // Indented line belongs to scalar block
                    if sub_line.starts_with("  ") || sub_line.starts_with('\t') {
                        scalar_lines.push(sub_line.trim());
                        i += 1;
                    } else if sub_line.trim().is_empty() {
                        scalar_lines.push("");
                        i += 1;
                    } else {
                        break;
                    }
                }

                let joined = if is_folded {
                    scalar_lines.join(" ").trim().to_string()
                } else {
                    scalar_lines.join("\n").trim().to_string()
                };

                map.insert(key, joined);
                continue;
            } else {
                // Strip outer quotes if present
                if (val_part.starts_with('"') && val_part.ends_with('"') && val_part.len() >= 2)
                    || (val_part.starts_with('\'')
                        && val_part.ends_with('\'')
                        && val_part.len() >= 2)
                {
                    val_part = &val_part[1..val_part.len() - 1];
                }

                // Strip inline brackets for list: [a, b]
                let val_str = if val_part.starts_with('[') && val_part.ends_with(']') {
                    val_part[1..val_part.len() - 1].trim().to_string()
                } else {
                    val_part.to_string()
                };

                map.insert(key, val_str);
            }
        }

        i += 1;
    }

    let body = lines[(end_idx + 1)..].join("\n").trim().to_string();
    (map, body)
}

/// Fallback helper to extract the first non-header text paragraph as a description.
fn extract_first_paragraph(content: &str) -> String {
    for para in content.split("\n\n") {
        let trimmed = para.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            return trimmed.lines().next().unwrap_or(trimmed).to_string();
        }
    }
    "No description provided.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_with_frontmatter() {
        let raw = r#"---
name: automation
description: Interactive guide to design and create a scheduled background automation.
command: /automation
metadata:
  icon: schedule
tags: [workflow, automation]
---

# Scheduled Automations
Documentation content goes here.
"#;
        let skill =
            SkillRegistry::parse_skill_content(raw, PathBuf::from("dummy/SKILL.md")).unwrap();
        assert_eq!(skill.name, "automation");
        assert_eq!(
            skill.description,
            "Interactive guide to design and create a scheduled background automation."
        );
        assert_eq!(skill.command, "/automation");
        assert_eq!(skill.tags, vec!["workflow", "automation"]);
        assert!(skill.content.contains("# Scheduled Automations"));
    }

    #[test]
    fn test_parse_skill_with_folded_scalar_description() {
        let raw = r#"---
name: workspace-workflow
description: >-
  Procedures for building, testing, and debugging
  workspace packages on Windows. Covers toolchain activation,
  cargo release testing, and token DB safety guards.
---

# Workflow Body
"#;
        let skill =
            SkillRegistry::parse_skill_content(raw, PathBuf::from("workflow/SKILL.md")).unwrap();
        assert_eq!(skill.name, "workspace-workflow");
        assert!(skill
            .description
            .contains("Procedures for building, testing, and debugging"));
        assert!(skill.description.contains("token DB safety guards."));
        assert_eq!(skill.command, "/workspace-workflow");
    }

    #[test]
    fn test_skill_registry_dispatch_and_resolution() {
        let mut registry = SkillRegistry::new();

        let s1 = Skill {
            name: "plan".to_string(),
            description: "Planning mode for complex tasks".to_string(),
            command: "/plan".to_string(),
            metadata: HashMap::new(),
            tags: vec![],
            content: "Plan instructions".to_string(),
            path: PathBuf::from("skills/plan/SKILL.md"),
        };

        let s2 = Skill {
            name: "benchmark".to_string(),
            description: "Programmatic tool synthesis and stress testing".to_string(),
            command: "/benchmark".to_string(),
            metadata: HashMap::new(),
            tags: vec![],
            content: "Benchmark instructions".to_string(),
            path: PathBuf::from("skills/benchmark/SKILL.md"),
        };

        registry.register(s1);
        registry.register(s2);

        assert_eq!(registry.len(), 2);

        // Lookup by command with and without slash
        let resolved = registry.get_skill_by_command("/plan").unwrap();
        assert_eq!(resolved.name, "plan");

        let resolved_no_slash = registry.get_skill_by_command("benchmark").unwrap();
        assert_eq!(resolved_no_slash.name, "benchmark");

        let missing = registry.get_skill_by_command("/nonexistent");
        assert!(missing.is_none());
    }

    #[test]
    fn test_format_slash_commands_prompt() {
        let mut registry = SkillRegistry::new();
        registry.register(Skill {
            name: "goal".to_string(),
            description: "Run long-running autonomous tasks".to_string(),
            command: "/goal".to_string(),
            metadata: HashMap::new(),
            tags: vec![],
            content: "".to_string(),
            path: PathBuf::from("skills/goal/SKILL.md"),
        });

        let prompt = registry.format_slash_commands_prompt();
        assert!(prompt.contains("Available slash commands you can recommend to the user:"));
        assert!(prompt.contains("- /goal: Run long-running autonomous tasks"));
    }

    #[tokio::test]
    async fn test_discover_skills_from_directory() {
        let temp_dir =
            std::env::temp_dir().join(format!("agenda_skills_disc_{}", std::process::id()));
        let skill_dir = temp_dir.join(".agenda").join("skills").join("my_tool");
        fs::create_dir_all(&skill_dir).unwrap();

        let skill_file = skill_dir.join("SKILL.md");
        fs::write(
            &skill_file,
            r#"---
name: my-tool
description: Custom test skill
command: /my-tool
---
# Tool Doc
"#,
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        let count = registry.discover_skills(&temp_dir).await.unwrap();
        assert!(count >= 1);

        let retrieved = registry.get_skill_by_command("/my-tool");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "my-tool");

        let _ = fs::remove_dir_all(temp_dir);
    }
}
