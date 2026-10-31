use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Input arguments for the `save_skill` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SaveSkillArgs {
    /// Unique identifier for the skill (e.g. 'benchmark-profiler', 'git-workflow').
    pub name: String,
    /// Short description of what the skill does and when the agent should activate it.
    pub description: String,
    /// Markdown documentation, instructions, or step-by-step guidelines.
    pub content: String,
    /// Whether to permit overwriting an existing skill with the same name.
    #[serde(default)]
    pub overwrite: bool,
}

/// Output metadata returned after saving a skill.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SaveSkillResult {
    /// Skill name.
    pub name: String,
    /// Absolute or workspace-relative path where `SKILL.md` was created.
    pub path: PathBuf,
    /// Number of bytes written to the file.
    pub bytes_written: usize,
    /// Whether the action received user approval.
    pub approved: bool,
}

/// Validates that a skill name is safe for disk directory creation.
pub fn sanitize_skill_name(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Skill name cannot be empty");
    }

    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        anyhow::bail!(
            "Skill name '{}' contains illegal path traversal characters",
            trimmed
        );
    }

    // Must consist of alphanumeric characters, hyphens, or underscores
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Skill name '{}' contains invalid characters. Use alphanumeric characters, hyphens, or underscores.",
            trimmed
        );
    }

    Ok(trimmed.to_ascii_lowercase())
}

/// Constructs complete markdown text with valid YAML frontmatter.
pub fn format_skill_document(name: &str, description: &str, content: &str) -> String {
    let clean_content = content.trim();
    if clean_content.starts_with("---") {
        // Content already contains frontmatter; retain as-is
        clean_content.to_string()
    } else {
        format!(
            "---\nname: {}\ndescription: {}\n---\n\n{}\n",
            name, description, clean_content
        )
    }
}

/// Returns the standard OpenAI/Anthropic/Gemini tool definition schema for `save_skill`.
pub fn save_skill_tool_definition() -> serde_json::Value {
    serde_json::json!({
        "name": "save_skill",
        "description": "Save a reusable skill to .agenda/skills/<name>/SKILL.md with user approval. Use this when a useful workflow or protocol should be persisted for future turns.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique identifier for the skill (e.g. 'benchmark-profiler', 'git-workflow')"
                },
                "description": {
                    "type": "string",
                    "description": "Short explanation of what the skill does and when to activate it"
                },
                "content": {
                    "type": "string",
                    "description": "Complete markdown instructions, protocols, or guidelines"
                }
            },
            "required": ["name", "description", "content"]
        }
    })
}

/// Persistent skill creator managing directory scaffolding, formatting, and safety approval.
pub struct SkillSaver {
    workspace_dir: PathBuf,
}

impl SkillSaver {
    /// Initialize a new `SkillSaver` targeting the given workspace.
    pub fn new(workspace_dir: impl AsRef<Path>) -> Self {
        Self {
            workspace_dir: workspace_dir.as_ref().to_path_buf(),
        }
    }

    /// Resolve destination path for the skill (`.agenda/skills/<name>/SKILL.md`).
    pub fn resolve_skill_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        let safe_name = sanitize_skill_name(name)?;
        Ok(self
            .workspace_dir
            .join(".agenda")
            .join("skills")
            .join(safe_name)
            .join("SKILL.md"))
    }

    /// Save the skill directly to disk without checking approval.
    pub fn save_skill_direct(&self, args: &SaveSkillArgs) -> anyhow::Result<SaveSkillResult> {
        let safe_name = sanitize_skill_name(&args.name)?;
        let target_path = self.resolve_skill_path(&safe_name)?;

        if target_path.exists() && !args.overwrite {
            anyhow::bail!(
                "Skill '{}' already exists at '{}'. Overwriting an existing skill requires explicit overwrite approval.",
                safe_name,
                target_path.display()
            );
        }

        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let document = format_skill_document(&safe_name, &args.description, &args.content);
        fs::write(&target_path, &document)?;

        Ok(SaveSkillResult {
            name: safe_name,
            path: target_path,
            bytes_written: document.len(),
            approved: true,
        })
    }

    /// Save the skill to disk only after verifying user approval via an asynchronous approval gate.
    pub async fn save_skill_with_approval<F, Fut>(
        &self,
        args: &SaveSkillArgs,
        approval_gate: F,
    ) -> anyhow::Result<SaveSkillResult>
    where
        F: FnOnce(&str, &str) -> Fut,
        Fut: Future<Output = bool>,
    {
        let safe_name = sanitize_skill_name(&args.name)?;
        let target_path = self.resolve_skill_path(&safe_name)?;
        let action = format!("save_skill: {}", safe_name);
        let reason = format!(
            "Creating persistent skill at {}",
            target_path
                .strip_prefix(&self.workspace_dir)
                .unwrap_or(&target_path)
                .display()
        );

        let approved = approval_gate(&action, &reason).await;
        if !approved {
            anyhow::bail!(
                "Skill creation for '{}' was declined by user approval gate",
                safe_name
            );
        }

        self.save_skill_direct(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_skill_name() {
        assert_eq!(sanitize_skill_name("My-Skill_1").unwrap(), "my-skill_1");
        assert!(sanitize_skill_name("../escape").is_err());
        assert!(sanitize_skill_name("bad/slash").is_err());
        assert!(sanitize_skill_name("").is_err());
        assert!(sanitize_skill_name("has spaces").is_err());
    }

    #[test]
    fn test_format_skill_document() {
        let doc = format_skill_document("test-skill", "Does testing", "Step 1\nStep 2");
        assert!(doc.starts_with("---\nname: test-skill\ndescription: Does testing\n---\n"));
        assert!(doc.contains("Step 1\nStep 2"));

        // If frontmatter is already included, preserves it
        let existing = "---\nname: custom\ndescription: desc\n---\nBody";
        assert_eq!(format_skill_document("custom", "desc", existing), existing);
    }

    #[tokio::test]
    async fn test_save_skill_with_approval_gate() -> anyhow::Result<()> {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_save_skill_{}", uuid::Uuid::new_v4()));
        let saver = SkillSaver::new(&temp_dir);

        let mut args = SaveSkillArgs {
            name: "test-recorder".to_string(),
            description: "Records diagnostic metrics".to_string(),
            content: "## Instructions\nRun probe and record stats.".to_string(),
            overwrite: false,
        };

        // 1. Rejection case
        let err = saver
            .save_skill_with_approval(&args, |_action, _reason| async { false })
            .await;
        assert!(err.is_err());
        let target_path = saver.resolve_skill_path("test-recorder")?;
        assert!(!target_path.exists());

        // 2. Approval case
        let res = saver
            .save_skill_with_approval(&args, |_action, _reason| async { true })
            .await?;
        assert!(res.approved);
        assert!(target_path.exists());

        let loaded = fs::read_to_string(&target_path)?;
        assert!(loaded.contains("name: test-recorder"));
        assert!(loaded.contains("Records diagnostic metrics"));

        // 3. Overwrite without overwrite flag must fail
        args.description = "Updated description".to_string();
        let overwrite_err = saver
            .save_skill_with_approval(&args, |_action, _reason| async { true })
            .await;
        assert!(overwrite_err.is_err());
        assert!(overwrite_err
            .unwrap_err()
            .to_string()
            .contains("already exists"));

        // 4. Overwrite with explicit overwrite flag succeeds
        args.overwrite = true;
        let overwrite_ok = saver
            .save_skill_with_approval(&args, |_action, _reason| async { true })
            .await?;
        assert!(overwrite_ok.approved);
        let updated = fs::read_to_string(&target_path)?;
        assert!(updated.contains("Updated description"));

        let _ = fs::remove_dir_all(temp_dir);
        Ok(())
    }
}
