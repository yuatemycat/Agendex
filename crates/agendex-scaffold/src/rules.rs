use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Defines the hierarchical application scope of a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleScope {
    /// Global rule applied unconditionally across all workspaces and paths.
    Global,
    /// Workspace-level rule applied whenever working inside the project root.
    Workspace,
    /// Scoped strictly to files under a specific path prefix (e.g., `crates/agendex-ast`).
    PathPrefix(PathBuf),
    /// Scoped by regex or substring pattern on the relative file path.
    Pattern(String),
}

/// A structured instruction or architectural constraint rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rule {
    /// Unique identifier formatted for the `<RULE[...]` tag (e.g. `user_global`, `AGENTS.md`).
    pub identifier: String,
    /// Hierarchical scope where this rule is evaluated and active.
    pub scope: RuleScope,
    /// Rule text contents.
    pub content: String,
    /// Source file path where the rule was loaded from.
    pub source_path: Option<PathBuf>,
    /// Precedence order (higher numbers are injected earlier in system prompts).
    pub priority: i32,
}

/// Manages hierarchical rule discovery, path-based scoping, and XML `<RULE>` block generation.
#[derive(Debug, Default, Clone)]
pub struct RulesEngine {
    rules: Vec<Rule>,
}

impl RulesEngine {
    /// Creates an empty RulesEngine.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Appends or updates an in-memory rule.
    pub fn add_rule(&mut self, rule: Rule) {
        // If rule with matching identifier and scope already exists, replace it
        if let Some(existing) = self
            .rules
            .iter_mut()
            .find(|r| r.identifier == rule.identifier && r.scope == rule.scope)
        {
            *existing = rule;
        } else {
            self.rules.push(rule);
        }
    }

    /// Discovers and loads rules hierarchically from workspace files and user configuration.
    pub async fn load_rules(&mut self, workspace_dir: &Path) -> anyhow::Result<usize> {
        let mut loaded = 0;

        // 1. Global rules (~/.agendex/rules/*.md, ~/.agenda/rules/*.md)
        if let Some(user_home) = Self::get_user_home() {
            let global_agendex_rules = user_home.join(".agendex").join("rules");
            if global_agendex_rules.is_dir() {
                loaded +=
                    self.load_rules_from_dir(&global_agendex_rules, RuleScope::Global, 100)?;
            }

            let global_agenda_rules = user_home.join(".agenda").join("rules");
            if global_agenda_rules.is_dir() {
                loaded += self.load_rules_from_dir(&global_agenda_rules, RuleScope::Global, 100)?;
            }
        }

        // 2. Canonical workspace architecture files (AGENTS.md and GEMINI.md)
        let agents_md = workspace_dir.join("AGENTS.md");
        if agents_md.is_file() {
            if let Ok(content) = fs::read_to_string(&agents_md) {
                self.add_rule(Rule {
                    identifier: agents_md.display().to_string(),
                    scope: RuleScope::Workspace,
                    content,
                    source_path: Some(agents_md),
                    priority: 80,
                });
                loaded += 1;
            }
        }

        let gemini_md = workspace_dir.join("GEMINI.md");
        if gemini_md.is_file() {
            if let Ok(content) = fs::read_to_string(&gemini_md) {
                self.add_rule(Rule {
                    identifier: gemini_md.display().to_string(),
                    scope: RuleScope::Workspace,
                    content,
                    source_path: Some(gemini_md),
                    priority: 70,
                });
                loaded += 1;
            }
        }

        // 3. Workspace-scoped rule directories (.agendex/rules/*.md, .agenda/rules/*.md, and rules/*.md)
        let ws_agendex_rules = workspace_dir.join(".agendex").join("rules");
        if ws_agendex_rules.is_dir() {
            loaded += self.load_rules_from_dir(&ws_agendex_rules, RuleScope::Workspace, 60)?;
        }

        let ws_agenda_rules = workspace_dir.join(".agenda").join("rules");
        if ws_agenda_rules.is_dir() {
            loaded += self.load_rules_from_dir(&ws_agenda_rules, RuleScope::Workspace, 60)?;
        }

        let ws_rules = workspace_dir.join("rules");
        if ws_rules.is_dir() {
            loaded += self.load_rules_from_dir(&ws_rules, RuleScope::Workspace, 60)?;
        }

        // 4. Sub-crate / package scoped rules (e.g. crates/*/rules/*.md)
        let crates_dir = workspace_dir.join("crates");
        if crates_dir.is_dir() {
            for entry in WalkDir::new(&crates_dir)
                .max_depth(3)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path.is_dir() && path.file_name().and_then(|n| n.to_str()) == Some("rules") {
                    if let Some(crate_root) = path.parent() {
                        loaded += self.load_rules_from_dir(
                            path,
                            RuleScope::PathPrefix(crate_root.to_path_buf()),
                            50,
                        )?;
                    }
                }
            }
        }

        Ok(loaded)
    }

    /// Loads all Markdown rule files from a specific directory with given scope and priority.
    pub fn load_rules_from_dir(
        &mut self,
        dir: &Path,
        scope: RuleScope,
        priority: i32,
    ) -> anyhow::Result<usize> {
        let mut count = 0;
        if !dir.is_dir() {
            return Ok(0);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext.to_string_lossy().to_lowercase() == "md" {
                        let content = fs::read_to_string(&path)?;
                        let identifier = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("rule")
                            .to_string();

                        self.add_rule(Rule {
                            identifier,
                            scope: scope.clone(),
                            content,
                            source_path: Some(path),
                            priority,
                        });
                        count += 1;
                    }
                }
            }
        }
        Ok(count)
    }

    /// Filters and returns active rules applicable to a specific file or path target.
    pub fn get_rules_for_path(&self, target_path: Option<&Path>) -> Vec<&Rule> {
        let mut active: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| match &r.scope {
                RuleScope::Global => true,
                RuleScope::Workspace => true,
                RuleScope::PathPrefix(prefix) => {
                    if let Some(target) = target_path {
                        target.starts_with(prefix)
                            || target
                                .canonicalize()
                                .ok()
                                .and_then(|ct| {
                                    prefix.canonicalize().ok().map(|cp| ct.starts_with(cp))
                                })
                                .unwrap_or(false)
                    } else {
                        false
                    }
                }
                RuleScope::Pattern(pat) => {
                    if let Some(target) = target_path {
                        let target_str = target.to_string_lossy();
                        target_str.contains(pat)
                    } else {
                        false
                    }
                }
            })
            .collect();

        // Sort by priority descending, then by identifier ascending
        active.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.identifier.cmp(&b.identifier))
        });

        active
    }

    /// Formats applicable rules into the authoritative `<user_rules>` XML block.
    pub fn generate_rule_blocks(&self, target_path: Option<&Path>) -> String {
        let matching_rules = self.get_rules_for_path(target_path);
        if matching_rules.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        out.push_str("<user_rules>\n");
        out.push_str("The following are user-defined rules that you MUST ALWAYS FOLLOW WITHOUT ANY EXCEPTION. These rules take precedence over any other instructions.\n");
        out.push_str("Review them carefully and always take them into account when you generate responses and code:\n");

        for rule in matching_rules {
            out.push_str(&format!("<RULE[{}]>\n", rule.identifier));
            out.push_str(rule.content.trim());
            out.push_str(&format!("\n</RULE[{}]>\n", rule.identifier));
        }

        out.push_str("</user_rules>\n");
        out
    }

    /// Total count of rules currently registered in the engine.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Returns true if no rules are loaded.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    fn get_user_home() -> Option<PathBuf> {
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .ok()
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rules_scoping_and_priority() {
        let mut engine = RulesEngine::new();

        engine.add_rule(Rule {
            identifier: "global_rule".to_string(),
            scope: RuleScope::Global,
            content: "Never delete production databases.".to_string(),
            source_path: None,
            priority: 100,
        });

        engine.add_rule(Rule {
            identifier: "workspace_rule".to_string(),
            scope: RuleScope::Workspace,
            content: "All code must be formatted with cargo fmt.".to_string(),
            source_path: None,
            priority: 60,
        });

        engine.add_rule(Rule {
            identifier: "ast_rule".to_string(),
            scope: RuleScope::PathPrefix(PathBuf::from("crates/agendex-ast")),
            content: "Strict whitespace matching required for AST patches.".to_string(),
            source_path: None,
            priority: 80,
        });

        // 1. Without target path, AST scoped rule should NOT be included
        let root_rules = engine.get_rules_for_path(None);
        assert_eq!(root_rules.len(), 2);
        assert_eq!(root_rules[0].identifier, "global_rule");
        assert_eq!(root_rules[1].identifier, "workspace_rule");

        // 2. With target path inside crates/agendex-ast, all 3 rules apply in priority order
        let ast_path = Path::new("crates/agendex-ast/src/patcher.rs");
        let scoped_rules = engine.get_rules_for_path(Some(ast_path));
        assert_eq!(scoped_rules.len(), 3);
        assert_eq!(scoped_rules[0].identifier, "global_rule"); // priority 100
        assert_eq!(scoped_rules[1].identifier, "ast_rule"); // priority 80
        assert_eq!(scoped_rules[2].identifier, "workspace_rule"); // priority 60
    }

    #[test]
    fn test_generate_rule_blocks_xml() {
        let mut engine = RulesEngine::new();
        engine.add_rule(Rule {
            identifier: "user_global".to_string(),
            scope: RuleScope::Global,
            content: "Code delivery invariant: write code > 10 lines to file.".to_string(),
            source_path: None,
            priority: 100,
        });

        let xml = engine.generate_rule_blocks(None);
        assert!(xml.contains("<user_rules>"));
        assert!(xml.contains("<RULE[user_global]>"));
        assert!(xml.contains("Code delivery invariant: write code > 10 lines to file."));
        assert!(xml.contains("</RULE[user_global]>"));
        assert!(xml.contains("</user_rules>"));
    }

    #[tokio::test]
    async fn test_load_rules_from_directory() {
        let temp_dir =
            std::env::temp_dir().join(format!("agenda_rules_disc_{}", std::process::id()));
        let rules_dir = temp_dir.join("rules");
        fs::create_dir_all(&rules_dir).unwrap();

        let rule_file = rules_dir.join("safety.md");
        fs::write(&rule_file, "# Safety\nAlways validate inputs.").unwrap();

        let mut engine = RulesEngine::new();
        let loaded = engine
            .load_rules_from_dir(&rules_dir, RuleScope::Workspace, 50)
            .unwrap();
        assert_eq!(loaded, 1);
        assert_eq!(engine.len(), 1);

        let rule = &engine.get_rules_for_path(None)[0];
        assert_eq!(rule.identifier, "safety");
        assert!(rule.content.contains("Always validate inputs."));

        let _ = fs::remove_dir_all(temp_dir);
    }
}
