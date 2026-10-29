use anyhow::Result;
use reedline::{
    Completer, CompletionResult, EditCommand, Emacs, FileBackedHistory, IdeMenu, KeyCode,
    KeyModifiers, Keybindings, MenuBuilder, Prompt, PromptEditMode, PromptHistorySearch, Reedline,
    ReedlineEvent, ReedlineMenu, Span, Suggestion,
};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SlashCommandInfo {
    pub name: String,
    pub description: String,
}

pub struct AgendexCompleter {
    pub workspace_dir: PathBuf,
    pub commands: Vec<SlashCommandInfo>,
}

impl AgendexCompleter {
    pub fn new(workspace_dir: PathBuf, commands: Vec<SlashCommandInfo>) -> Self {
        Self {
            workspace_dir,
            commands,
        }
    }
}

impl Completer for AgendexCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let prefix = &line[..pos];

        // 1. Slash command completion: starts with /
        if prefix.starts_with('/') && !prefix.contains(' ') {
            let filter = prefix.to_lowercase();
            let suggestions: Vec<Suggestion> = self
                .commands
                .iter()
                .filter(|c| c.name.to_lowercase().starts_with(&filter))
                .map(|c| Suggestion {
                    value: c.name.clone(),
                    display_override: None,
                    description: Some(c.description.clone()),
                    style: None,
                    extra: None,
                    span: Span {
                        start: 0,
                        end: pos,
                    },
                    append_whitespace: true,
                    match_indices: None,
                })
                .collect();
            return CompletionResult::fresh(suggestions);
        }

        // 2. @file path completion
        if let Some(at_idx) = prefix.rfind('@') {
            let file_prefix = &prefix[at_idx + 1..];
            if !file_prefix.contains(' ') {
                let mut suggestions = Vec::new();
                let search_dir = self.workspace_dir.clone();
                let walker = walkdir::WalkDir::new(&search_dir)
                    .max_depth(4)
                    .into_iter()
                    .filter_entry(|e| {
                        let name = e.file_name().to_string_lossy();
                        !name.starts_with('.') && name != "target" && name != "node_modules"
                    });

                for entry in walker.filter_map(|e| e.ok()) {
                    if entry.file_type().is_file() {
                        if let Ok(rel) = entry.path().strip_prefix(&self.workspace_dir) {
                            let rel_str = rel.to_string_lossy().replace('\\', "/");
                            if rel_str.to_lowercase().starts_with(&file_prefix.to_lowercase()) {
                                suggestions.push(Suggestion {
                                    value: format!("@{}", rel_str),
                                    display_override: None,
                                    description: Some(format!("Attach file ({})", rel_str)),
                                    style: None,
                                    extra: None,
                                    span: Span {
                                        start: at_idx,
                                        end: pos,
                                    },
                                    append_whitespace: true,
                                    match_indices: None,
                                });
                                if suggestions.len() >= 25 {
                                    break;
                                }
                            }
                        }
                    }
                }
                return CompletionResult::fresh(suggestions);
            }
        }

        CompletionResult::fresh(Vec::new())
    }
}

pub struct AgendexPrompt {
    pub model_name: String,
    pub git_branch: Option<String>,
    pub yolo_mode: bool,
}

impl Prompt for AgendexPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        let yolo_indicator = if self.yolo_mode {
            "\x1b[38;2;251;191;36m[YOLO]\x1b[0m "
        } else {
            ""
        };
        Cow::Owned(format!(
            "{}\x1b[1;38;2;129;140;248magendex\x1b[0m \x1b[38;2;56;189;248m❯\x1b[0m ",
            yolo_indicator
        ))
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        let branch_info = match &self.git_branch {
            Some(b) => format!(" \x1b[38;2;100;116;139mgit:\x1b[38;2;148;163;184m{}\x1b[0m", b),
            None => String::new(),
        };
        let model_info = if !self.model_name.is_empty() {
            let short_model = self.model_name.split('/').next_back().unwrap_or(&self.model_name);
            format!("\x1b[38;2;100;116;139m[{}]\x1b[0m", short_model)
        } else {
            String::new()
        };
        Cow::Owned(format!("{}{}", model_info, branch_info))
    }

    fn render_prompt_indicator(&self, _prompt_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("\x1b[38;2;100;116;139m::: \x1b[0m")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        Cow::Owned(format!(
            "\x1b[38;2;56;189;248m(reverse-i-search)`{}': \x1b[0m",
            history_search.term
        ))
    }
}

pub fn create_reedline_editor(
    workspace_dir: &Path,
    commands: Vec<SlashCommandInfo>,
) -> Result<Reedline> {
    // 1. History directory ~/.agendex/history.txt
    let home_dir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let agendex_dir = PathBuf::from(home_dir).join(".agendex");
    let _ = std::fs::create_dir_all(&agendex_dir);
    let history_file = agendex_dir.join("history.txt");

    let history = Box::new(
        FileBackedHistory::with_file(2000, history_file)
            .map_err(|e| anyhow::anyhow!("Failed to open history: {}", e))?,
    );

    // 2. Completer & IdeMenu
    let completer = Box::new(AgendexCompleter::new(workspace_dir.to_path_buf(), commands));
    let ide_menu = Box::new(
        IdeMenu::default()
            .with_name("ide_menu")
            .with_min_completion_width(20)
            .with_max_completion_width(60)
            .with_max_completion_height(10),
    );

    // 3. Keybindings
    let mut keybindings = Keybindings::new();
    // Tab triggers / cycles completion menu
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("ide_menu".into()),
            ReedlineEvent::MenuNext,
        ]),
    );
    // Shift-Tab cycles backward
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
    // Alt-Enter inserts a newline
    keybindings.add_binding(
        KeyModifiers::ALT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    // Ctrl-R initiates reverse history search
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('r'),
        ReedlineEvent::SearchHistory,
    );

    let edit_mode = Box::new(Emacs::new(keybindings));

    let line_editor = Reedline::create()
        .with_history(history)
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(ide_menu))
        .with_edit_mode(edit_mode);

    Ok(line_editor)
}

/// Detects the active Git branch name for the prompt.
pub fn detect_git_branch(workspace_dir: &Path) -> Option<String> {
    let head_file = workspace_dir.join(".git").join("HEAD");
    if let Ok(content) = std::fs::read_to_string(head_file) {
        let trimmed = content.trim();
        if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
            return Some(branch.to_string());
        }
    }
    None
}

/// Expands `@path/to/file` mentions by reading the file and appending an attachment block.
pub fn expand_file_mentions(workspace_dir: &Path, input: &str) -> String {
    let mut expanded = input.to_string();
    let mut attachments = Vec::new();

    for word in input.split_whitespace() {
        if let Some(path_str) = word.strip_prefix('@') {
            if path_str.is_empty() {
                continue;
            }
            // Strip any trailing punctuation (e.g. @file.rs, or @file.rs:)
            let clean_path = path_str.trim_matches(|c| c == ',' || c == ':' || c == ';');
            let target = workspace_dir.join(clean_path);
            if target.is_file() {
                if let Ok(content) = std::fs::read_to_string(&target) {
                    let preview = if content.len() > 30_000 {
                        format!("{}...\n[Truncated - file exceeds 30KB]", &content[..30_000])
                    } else {
                        content
                    };
                    attachments.push(format!(
                        "\n--- [Attached File: {}] ---\n{}\n--- [End of {}] ---",
                        clean_path, preview, clean_path
                    ));
                }
            }
        }
    }

    if !attachments.is_empty() {
        expanded.push_str("\n\nAttached Context from Workspace:");
        for att in attachments {
            expanded.push_str(&att);
        }
    }

    expanded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_file_mentions() {
        let temp_dir = std::env::temp_dir().join(format!("agendex_expand_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let test_file = temp_dir.join("sample.txt");
        std::fs::write(&test_file, "hello from sample file!").unwrap();

        let input = "Check out @sample.txt please";
        let expanded = expand_file_mentions(&temp_dir, input);

        assert!(expanded.contains("--- [Attached File: sample.txt] ---"));
        assert!(expanded.contains("hello from sample file!"));
        assert!(expanded.contains("--- [End of sample.txt] ---"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_completer_slash_commands() {
        let commands = vec![
            SlashCommandInfo {
                name: "/help".into(),
                description: "Help command".into(),
            },
            SlashCommandInfo {
                name: "/model".into(),
                description: "Model command".into(),
            },
        ];
        let mut completer = AgendexCompleter::new(PathBuf::from("."), commands);
        let res = completer.complete("/m", 2);
        match res {
            reedline::CompletionResult::Fresh { suggestions, .. } => {
                assert_eq!(suggestions.len(), 1);
                assert_eq!(suggestions[0].value, "/model");
            }
            _ => panic!("Expected fresh completion result"),
        }
    }

    #[test]
    fn test_prompt_rendering() {
        let prompt = AgendexPrompt {
            model_name: "meta/llama-3.1-70b-instruct".into(),
            git_branch: Some("main".into()),
            yolo_mode: true,
        };
        let left = prompt.render_prompt_left();
        let right = prompt.render_prompt_right();

        assert!(left.contains("[YOLO]"));
        assert!(left.contains("agendex"));
        assert!(right.contains("llama-3.1-70b-instruct"));
        assert!(right.contains("git:"));
        assert!(right.contains("main"));
    }
}
