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
                    span: Span { start: 0, end: pos },
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
                            if rel_str
                                .to_lowercase()
                                .starts_with(&file_prefix.to_lowercase())
                            {
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
            Some(b) => format!(
                " \x1b[38;2;100;116;139mgit:\x1b[38;2;148;163;184m{}\x1b[0m",
                b
            ),
            None => String::new(),
        };
        let model_info = if !self.model_name.is_empty() {
            let short_model = self
                .model_name
                .split('/')
                .next_back()
                .unwrap_or(&self.model_name);
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

/// Sanitizes history commands to prevent persisting sensitive API keys in ~/.agendex/history.txt
pub fn sanitize_history_line(line: &str) -> String {
    let trimmed = line.trim();
    let mut scrubbed =
        if trimmed.starts_with("/provider add") || trimmed.starts_with("/providers add") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(parsed) = agendex_core::config::parse_provider_add_tokens(&parts) {
                if let Some(ref key) = parsed.api_key {
                    if !key.starts_with('$') {
                        let mut new_parts = Vec::new();
                        let mut replaced = false;
                        for part in &parts {
                            if !replaced && *part == key.as_str() {
                                new_parts.push("[REDACTED]");
                                replaced = true;
                            } else {
                                new_parts.push(*part);
                            }
                        }
                        new_parts.join(" ")
                    } else {
                        trimmed.to_string()
                    }
                } else {
                    trimmed.to_string()
                }
            } else {
                trimmed.to_string()
            }
        } else {
            line.to_string()
        };

    // Apply global pattern-based secret masking on all history entries
    scrubbed = agendex_trajectory::mask_secrets(&scrubbed);
    scrubbed
}

/// History wrapper that strips plain-text credentials before persisting.
pub struct SanitizedHistory {
    inner: Box<dyn reedline::History>,
}

impl SanitizedHistory {
    pub fn new(inner: Box<dyn reedline::History>) -> Self {
        Self { inner }
    }
}

impl reedline::History for SanitizedHistory {
    fn save(&mut self, mut h: reedline::HistoryItem) -> reedline::Result<reedline::HistoryItem> {
        h.command_line = sanitize_history_line(&h.command_line);
        self.inner.save(h)
    }

    fn load(&self, id: reedline::HistoryItemId) -> reedline::Result<reedline::HistoryItem> {
        self.inner.load(id)
    }

    fn count(&self, query: reedline::SearchQuery) -> reedline::Result<i64> {
        self.inner.count(query)
    }

    fn search(&self, query: reedline::SearchQuery) -> reedline::Result<Vec<reedline::HistoryItem>> {
        self.inner.search(query)
    }

    fn update(
        &mut self,
        id: reedline::HistoryItemId,
        entry: &dyn Fn(reedline::HistoryItem) -> reedline::HistoryItem,
    ) -> reedline::Result<()> {
        let sanitized_entry = |item: reedline::HistoryItem| {
            let mut transformed = entry(item);
            transformed.command_line = sanitize_history_line(&transformed.command_line);
            transformed
        };
        self.inner.update(id, &sanitized_entry)
    }

    fn clear(&mut self) -> reedline::Result<()> {
        self.inner.clear()
    }

    fn delete(&mut self, h: reedline::HistoryItemId) -> reedline::Result<()> {
        self.inner.delete(h)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.inner.sync()
    }

    fn session(&self) -> Option<reedline::HistorySessionId> {
        self.inner.session()
    }

    fn count_all(&self) -> reedline::Result<i64> {
        self.inner.count_all()
    }
}

/// Constructs default keybindings including emacs defaults (Enter, Backspace, arrows, Ctrl-C, Ctrl-D)
/// augmented with Tab for completion menu, Alt-Enter for multiline newline, and Ctrl-R for search.
pub fn create_default_keybindings() -> Keybindings {
    let mut keybindings = reedline::default_emacs_keybindings();

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

    // Typing '@' inserts '@' and immediately activates the @file completion menu
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Char('@'),
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::InsertChar('@')]),
            ReedlineEvent::Menu("ide_menu".into()),
        ]),
    );

    keybindings
}

pub fn create_reedline_editor(
    workspace_dir: &Path,
    commands: Vec<SlashCommandInfo>,
) -> Result<Reedline> {
    // 1. History directory ~/.agendex/history.txt with restrictive permissions
    let home_dir = std::env::var("AGENDEX_HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let agendex_dir = PathBuf::from(home_dir).join(".agendex");
    let _ = std::fs::create_dir_all(&agendex_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&agendex_dir, std::fs::Permissions::from_mode(0o700));
    }

    let history_file = agendex_dir.join("history.txt");
    if !history_file.exists() {
        let _ = std::fs::write(&history_file, "");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&history_file, std::fs::Permissions::from_mode(0o600));
    }

    let file_history = Box::new(
        FileBackedHistory::with_file(2000, history_file)
            .map_err(|e| anyhow::anyhow!("Failed to open history: {}", e))?,
    );
    let history = Box::new(SanitizedHistory::new(file_history));

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
    let keybindings = create_default_keybindings();
    let edit_mode = Box::new(Emacs::new(keybindings));

    let line_editor = Reedline::create()
        .with_history(history)
        .with_completer(completer)
        .with_menu(ReedlineMenu::EngineCompleter(ide_menu))
        .with_edit_mode(edit_mode);

    Ok(line_editor)
}

/// Helper to test subpath inclusion portably across Unix and Windows.
fn is_subpath(path: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let p_str = path.to_string_lossy().to_lowercase().replace('/', "\\");
        let b_str = base.to_string_lossy().to_lowercase().replace('/', "\\");
        if p_str == b_str {
            return true;
        }
        let prefix = if b_str.ends_with('\\') {
            b_str
        } else {
            format!("{}\\", b_str)
        };
        p_str.starts_with(&prefix)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(base)
    }
}

fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
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
/// Rejects paths outside the workspace, directory traversal, and external symlink escapes.
/// Uses character boundary safe slicing to prevent panicking on multibyte UTF-8 files.
pub fn expand_file_mentions(workspace_dir: &Path, input: &str) -> String {
    let mut expanded = input.to_string();
    let mut attachments = Vec::new();

    let canon_ws = workspace_dir
        .canonicalize()
        .map(|p| strip_verbatim(&p))
        .unwrap_or_else(|_| strip_verbatim(workspace_dir));

    for word in input.split_whitespace() {
        if let Some(path_str) = word.strip_prefix('@') {
            if path_str.is_empty() {
                continue;
            }
            // Strip any trailing punctuation (e.g. @file.rs, or @file.rs:)
            let clean_path = path_str.trim_matches(|c| c == ',' || c == ':' || c == ';');

            // Explicitly reject root escapes and absolute paths
            if clean_path.starts_with('/')
                || clean_path.starts_with('\\')
                || Path::new(clean_path).is_absolute()
            {
                continue;
            }

            let target = workspace_dir.join(clean_path);

            // Canonicalize target to resolve relative .. components and symlinks
            let canon_target = match target.canonicalize() {
                Ok(p) => strip_verbatim(&p),
                Err(_) => continue,
            };

            // Enforce workspace boundary: must resolve strictly to a subpath of canon_ws
            if !is_subpath(&canon_target, &canon_ws) {
                continue;
            }

            // Check if the target is a symlink pointing outside
            if let Ok(meta) = std::fs::symlink_metadata(&target) {
                if meta.file_type().is_symlink() && !is_subpath(&canon_target, &canon_ws) {
                    continue;
                }
            }

            if canon_target.is_file() {
                if let Ok(content) = std::fs::read_to_string(&canon_target) {
                    let preview = if content.len() > 30_000 {
                        let safe_idx = content.floor_char_boundary(30_000);
                        format!(
                            "{}...\n[Truncated - file exceeds 30KB]",
                            &content[..safe_idx]
                        )
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
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_expand_test_{}", uuid::Uuid::new_v4()));
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
    fn test_default_keybindings_presence() {
        let bindings = create_default_keybindings();
        // Enter -> Submit
        assert!(
            bindings
                .find_binding(KeyModifiers::NONE, KeyCode::Enter)
                .is_some(),
            "Enter key must be bound for command submission"
        );
        // Backspace
        assert!(
            bindings
                .find_binding(KeyModifiers::NONE, KeyCode::Backspace)
                .is_some(),
            "Backspace key must be bound for editing"
        );
        // Left & Right arrow navigation
        assert!(
            bindings
                .find_binding(KeyModifiers::NONE, KeyCode::Left)
                .is_some(),
            "Left arrow key must be bound"
        );
        assert!(
            bindings
                .find_binding(KeyModifiers::NONE, KeyCode::Right)
                .is_some(),
            "Right arrow key must be bound"
        );
        // Tab for completion menu
        assert!(
            bindings
                .find_binding(KeyModifiers::NONE, KeyCode::Tab)
                .is_some(),
            "Tab key must be bound for completion menu"
        );
        // Shift-Tab for cycling menu backward
        assert!(
            bindings
                .find_binding(KeyModifiers::SHIFT, KeyCode::BackTab)
                .is_some(),
            "Shift-Tab must be bound"
        );
        // Alt-Enter for multiline newline insertion
        assert!(
            bindings
                .find_binding(KeyModifiers::ALT, KeyCode::Enter)
                .is_some(),
            "Alt-Enter must be bound for multiline input"
        );
        // Ctrl-R for history search
        assert!(
            bindings
                .find_binding(KeyModifiers::CONTROL, KeyCode::Char('r'))
                .is_some(),
            "Ctrl-R must be bound for reverse history search"
        );
        // Ctrl-C
        assert!(
            bindings
                .find_binding(KeyModifiers::CONTROL, KeyCode::Char('c'))
                .is_some(),
            "Ctrl-C must be bound"
        );
        // Ctrl-D
        assert!(
            bindings
                .find_binding(KeyModifiers::CONTROL, KeyCode::Char('d'))
                .is_some(),
            "Ctrl-D must be bound"
        );
    }

    #[test]
    fn test_expand_file_mentions_rejection_of_external_files_and_traversal() {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_sec_test_{}", uuid::Uuid::new_v4()));
        let ws_dir = temp_dir.join("workspace");
        let outside_dir = temp_dir.join("outside");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::create_dir_all(&outside_dir).unwrap();

        let outside_file = outside_dir.join("passwd.txt");
        std::fs::write(&outside_file, "root:x:0:0:root").unwrap();

        let inside_file = ws_dir.join("valid.txt");
        std::fs::write(&inside_file, "valid workspace content").unwrap();

        // 1. Valid workspace file is attached
        let valid_input = "Inspect @valid.txt please";
        let valid_out = expand_file_mentions(&ws_dir, valid_input);
        assert!(valid_out.contains("--- [Attached File: valid.txt] ---"));
        assert!(valid_out.contains("valid workspace content"));

        // 2. Traversal relative path pointing outside is rejected
        let traversal_input = "Inspect @../../outside/passwd.txt please";
        let traversal_out = expand_file_mentions(&ws_dir, traversal_input);
        assert!(!traversal_out.contains("root:x:0:0:root"));

        // 3. Absolute path is rejected
        let abs_str = outside_file.to_string_lossy().to_string();
        let abs_input = format!("Inspect @{} please", abs_str);
        let abs_out = expand_file_mentions(&ws_dir, &abs_input);
        assert!(!abs_out.contains("root:x:0:0:root"));

        // 4. External symlink inside workspace pointing outside is rejected
        let symlink_path = ws_dir.join("link_to_passwd.txt");
        #[cfg(windows)]
        let symlink_created =
            std::os::windows::fs::symlink_file(&outside_file, &symlink_path).is_ok();
        #[cfg(not(windows))]
        let symlink_created = std::os::unix::fs::symlink(&outside_file, &symlink_path).is_ok();

        if symlink_created {
            let symlink_input = "Inspect @link_to_passwd.txt please";
            let symlink_out = expand_file_mentions(&ws_dir, symlink_input);
            assert!(
                !symlink_out.contains("root:x:0:0:root"),
                "Symlink pointing outside workspace must not be attached"
            );
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_expand_file_mentions_multibyte_utf8_truncation() {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_utf8_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create a file > 30KB filled with 3-byte and 4-byte UTF-8 characters (e.g. 🦀, 日本語)
        // 30,000 bytes falls squarely in the middle of a multibyte sequence.
        let mut multibyte_content = String::new();
        for _ in 0..8000 {
            multibyte_content.push_str("日本語🦀🔥");
        }
        let test_file = temp_dir.join("large_multibyte.txt");
        std::fs::write(&test_file, &multibyte_content).unwrap();

        let input = "Check @large_multibyte.txt";
        // Must NOT panic!
        let expanded = expand_file_mentions(&temp_dir, input);
        assert!(expanded.contains("[Truncated - file exceeds 30KB]"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_sanitize_history_line() {
        // Plaintext API key is scrubbed
        let raw = "/provider add custom-vllm http://127.0.0.1:8000/v1 custom-llm openai sk-live-secret12345";
        let sanitized = sanitize_history_line(raw);
        assert_eq!(
            sanitized,
            "/provider add custom-vllm http://127.0.0.1:8000/v1 custom-llm openai [REDACTED]"
        );

        // 5-arg form with unprefixed key (e.g. plainFAKEkey5arg) is also scrubbed
        let raw_5arg = "/provider add custom-vllm http://127.0.0.1:8000/v1 plainFAKEkey5arg";
        let sanitized_5arg = sanitize_history_line(raw_5arg);
        assert_eq!(
            sanitized_5arg,
            "/provider add custom-vllm http://127.0.0.1:8000/v1 [REDACTED]"
        );

        // Environment variable references are kept intact
        let env_raw =
            "/provider add custom-vllm http://127.0.0.1:8000/v1 custom-llm openai $VLLM_KEY";
        let env_sanitized = sanitize_history_line(env_raw);
        assert_eq!(env_sanitized, env_raw);

        // Normal commands untouched
        let normal = "/help";
        assert_eq!(sanitize_history_line(normal), normal);
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
