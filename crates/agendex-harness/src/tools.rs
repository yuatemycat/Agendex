use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tasks::{ManageTaskAction, RunCommandResult, TaskManager};

/// Known binary extensions rejected by `read_file` to prevent context window blowout.
const BINARY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "bin", "iso", "img", "png", "jpg", "jpeg", "webp", "gif", "ico",
    "bmp", "tiff", "pdf", "docx", "pptx", "xlsx", "zip", "tar", "gz", "7z", "rar", "wasm", "pyc",
    "class", "obj", "o", "a", "lib",
];

/// Maximum default bytes returned in a single `read_file` call (45 KB).
pub const DEFAULT_MAX_READ_BYTES: usize = 46_080;

/// Maximum default lines returned in a single `read_file` call.
pub const DEFAULT_MAX_READ_LINES: usize = 800;

// ============================================================================
// Tool Arguments and Results
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileArgs {
    pub path: PathBuf,
    pub start_line: Option<usize>, // 1-indexed
    pub end_line: Option<usize>,   // 1-indexed, inclusive
    pub byte_offset: Option<usize>,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileResult {
    pub path: PathBuf,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub bytes_read: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileArgs {
    pub path: PathBuf,
    pub content: String,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub append: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileResult {
    pub path: PathBuf,
    pub bytes_written: usize,
    pub created: bool,
    pub overwritten: bool,
    pub appended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListDirArgs {
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub recursive: bool,
    pub depth: Option<usize>,
    #[serde(default)]
    pub show_hidden: bool,
    pub max_entries: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub size_bytes: Option<u64>,
    pub modified: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirResult {
    pub path: PathBuf,
    pub entries: Vec<DirEntryInfo>,
    pub total_entries: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCommandArgs {
    #[serde(alias = "CommandLine", alias = "cmd")]
    pub command: String,
    pub wait_ms: Option<u64>,
    pub cwd: Option<PathBuf>,
    pub is_daemon: Option<bool>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManageTaskArgs {
    pub action: ManageTaskAction,
    pub task_id: Option<String>,
    pub input: Option<String>,
    pub tail_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobArgs {
    pub pattern: String,
    pub path: Option<PathBuf>,
    pub max_results: Option<usize>,
    #[serde(default)]
    pub show_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobResult {
    pub pattern: String,
    pub base_path: PathBuf,
    pub matches: Vec<PathBuf>,
    pub total_matches: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepArgs {
    pub pattern: String,
    pub path: Option<PathBuf>,
    pub glob: Option<String>,
    #[serde(default)]
    pub case_insensitive: bool,
    pub max_results: Option<usize>,
    pub context_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepMatch {
    pub path: PathBuf,
    pub line_number: usize,
    pub line_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_context: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_context: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepResult {
    pub pattern: String,
    pub matches: Vec<GrepMatch>,
    pub total_matches: usize,
    pub truncated: bool,
}

// ============================================================================
// ToolRegistry
// ============================================================================

fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path.to_path_buf()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut components = Vec::new();
    for c in path.components() {
        match c {
            Component::Prefix(p) => components.push(Component::Prefix(p)),
            Component::RootDir => components.push(Component::RootDir),
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                }
            }
            Component::Normal(c) => components.push(Component::Normal(c)),
        }
    }
    components.into_iter().collect()
}

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

/// Native Tool Registry providing sandboxed filesystem tools and process management.
pub struct ToolRegistry {
    workspace_dir: PathBuf,
    task_manager: Arc<TaskManager>,
}

impl ToolRegistry {
    pub fn new(workspace_dir: PathBuf, task_manager: Arc<TaskManager>) -> Self {
        Self {
            workspace_dir,
            task_manager,
        }
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn task_manager(&self) -> &Arc<TaskManager> {
        &self.task_manager
    }

    /// Resolves a path safely against workspace directory and verifies it does not escape.
    pub fn resolve_path(&self, p: &Path) -> Result<PathBuf> {
        let base = strip_verbatim(
            &self
                .workspace_dir
                .canonicalize()
                .unwrap_or_else(|_| normalize_path(&self.workspace_dir)),
        );
        let raw = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace_dir.join(p)
        };
        let normalized =
            strip_verbatim(&raw.canonicalize().unwrap_or_else(|_| normalize_path(&raw)));

        if !is_subpath(&normalized, &base) {
            return Err(anyhow!(
                "Path '{}' escapes workspace boundary '{}'",
                p.display(),
                self.workspace_dir.display()
            ));
        }
        Ok(normalized)
    }

    /// Reads a file with line-bound and byte-bound safeguards and binary rejection.
    pub async fn read_file(&self, args: ReadFileArgs) -> Result<ReadFileResult> {
        let resolved = self.resolve_path(&args.path)?;
        if !resolved.exists() {
            return Err(anyhow!("File not found: '{}'", resolved.display()));
        }
        if resolved.is_dir() {
            return Err(anyhow!(
                "Path is a directory, not a file: '{}'",
                resolved.display()
            ));
        }

        // Check file extension against prohibited binary list
        if let Some(ext) = resolved.extension().and_then(|e| e.to_str()) {
            let ext_lower = ext.to_ascii_lowercase();
            if BINARY_EXTENSIONS.contains(&ext_lower.as_str()) {
                let metadata = tokio::fs::metadata(&resolved).await.ok();
                let size = metadata.map(|m| m.len()).unwrap_or(0);
                return Err(anyhow!(
                    "Binary file reading prohibited for extension '.{}' at '{}' (size: {} bytes). Use specialized CLI probes instead.",
                    ext_lower,
                    resolved.display(),
                    size
                ));
            }
        }

        let raw_bytes = tokio::fs::read(&resolved)
            .await
            .with_context(|| format!("Failed to read file: {}", resolved.display()))?;

        // Null-byte check on first 512 bytes
        let probe_len = raw_bytes.len().min(512);
        if raw_bytes[..probe_len].contains(&0x00) {
            return Err(anyhow!(
                "Binary payload detected in file '{}' (contains null bytes). Reading prohibited to prevent context blowout.",
                resolved.display()
            ));
        }

        let full_text = String::from_utf8_lossy(&raw_bytes).to_string();
        let total_bytes = raw_bytes.len();
        let lines: Vec<&str> = full_text.lines().collect();
        let total_lines = lines.len();

        let max_lines = DEFAULT_MAX_READ_LINES;
        let max_bytes = args.max_bytes.unwrap_or(DEFAULT_MAX_READ_BYTES);

        let start_line = args.start_line.unwrap_or(1).max(1);
        let start_idx = start_line.saturating_sub(1);

        if start_idx >= total_lines && total_lines > 0 {
            return Ok(ReadFileResult {
                path: resolved,
                content: String::new(),
                start_line,
                end_line: start_line,
                total_lines,
                total_bytes,
                bytes_read: 0,
                truncated: false,
            });
        }

        let req_end_line = args.end_line.unwrap_or(start_line + max_lines - 1);
        let end_idx = req_end_line.min(total_lines).saturating_sub(1);

        let mut selected_lines = Vec::new();
        let mut bytes_accumulated = 0;
        let mut truncated = false;
        let mut actual_end_line = start_line;

        let byte_offset = args.byte_offset.unwrap_or(0);
        let mut current_offset = 0;

        for (idx, &line) in lines.iter().enumerate().take(end_idx + 1).skip(start_idx) {
            let line_bytes = line.len() + 1; // including newline
            current_offset += line_bytes;

            if current_offset <= byte_offset {
                continue;
            }

            if bytes_accumulated + line.len() > max_bytes || selected_lines.len() >= max_lines {
                truncated = true;
                break;
            }

            selected_lines.push(line);
            bytes_accumulated += line.len() + 1;
            actual_end_line = idx + 1;
        }

        if req_end_line < total_lines {
            truncated = true;
        }

        let content = selected_lines.join("\n");
        let bytes_read = content.len();

        Ok(ReadFileResult {
            path: resolved,
            content,
            start_line,
            end_line: actual_end_line,
            total_lines,
            total_bytes,
            bytes_read,
            truncated,
        })
    }

    /// Writes content to a file with strict overwrite/append safety checks.
    pub async fn write_file(&self, args: WriteFileArgs) -> Result<WriteFileResult> {
        let resolved = self.resolve_path(&args.path)?;

        if args.overwrite && args.append {
            return Err(anyhow!(
                "Cannot specify both overwrite=true and append=true"
            ));
        }

        if let Some(parent) = resolved.parent() {
            if !parent.exists() {
                tokio::fs::create_dir_all(parent).await.with_context(|| {
                    format!("Failed to create parent directory: {}", parent.display())
                })?;
            }
        }

        let exists = resolved.exists();
        let mut created = false;
        let mut overwritten = false;
        let mut appended = false;

        if exists {
            if !args.overwrite && !args.append {
                return Err(anyhow!(
                    "File '{}' already exists. Specify overwrite=true to replace or append=true to append.",
                    resolved.display()
                ));
            }

            if args.overwrite {
                tokio::fs::write(&resolved, args.content.as_bytes())
                    .await
                    .with_context(|| format!("Failed to overwrite file: {}", resolved.display()))?;
                overwritten = true;
            } else if args.append {
                use tokio::io::AsyncWriteExt;
                let mut file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&resolved)
                    .await
                    .with_context(|| {
                        format!("Failed to open file for append: {}", resolved.display())
                    })?;
                file.write_all(args.content.as_bytes()).await?;
                appended = true;
            }
        } else {
            tokio::fs::write(&resolved, args.content.as_bytes())
                .await
                .with_context(|| format!("Failed to write new file: {}", resolved.display()))?;
            created = true;
        }

        Ok(WriteFileResult {
            path: resolved,
            bytes_written: args.content.len(),
            created,
            overwritten,
            appended,
        })
    }

    /// Lists directory contents with recursion depth limit, hidden filter, and truncation bounds.
    pub async fn list_directory(&self, args: ListDirArgs) -> Result<ListDirResult> {
        let target_dir = match args.path {
            Some(ref p) => self.resolve_path(p)?,
            None => self.workspace_dir.clone(),
        };

        if !target_dir.exists() {
            return Err(anyhow!("Directory not found: '{}'", target_dir.display()));
        }
        if !target_dir.is_dir() {
            return Err(anyhow!(
                "Path is not a directory: '{}'",
                target_dir.display()
            ));
        }

        let max_entries = args.max_entries.unwrap_or(500);
        let max_depth = if args.recursive {
            args.depth.unwrap_or(5)
        } else {
            1
        };

        let mut entries = Vec::new();
        let mut truncated = false;

        let walker = walkdir::WalkDir::new(&target_dir)
            .max_depth(max_depth)
            .into_iter()
            .filter_entry(|e| {
                if !args.show_hidden {
                    let file_name = e.file_name().to_string_lossy();
                    if file_name.starts_with('.') && file_name != "." {
                        return false;
                    }
                }
                true
            });

        for item in walker {
            let entry = match item {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Skip root entry itself
            if entry.path() == target_dir {
                continue;
            }

            if entries.len() >= max_entries {
                truncated = true;
                break;
            }

            let metadata = entry.metadata().ok();
            let is_dir = entry.file_type().is_dir();
            let is_file = entry.file_type().is_file();
            let is_symlink = entry.file_type().is_symlink();
            let size_bytes = metadata.as_ref().map(|m| m.len());
            let modified = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from);

            let relative_path = entry
                .path()
                .strip_prefix(&self.workspace_dir)
                .unwrap_or_else(|_| entry.path())
                .to_path_buf();

            entries.push(DirEntryInfo {
                name: entry.file_name().to_string_lossy().to_string(),
                path: relative_path,
                is_dir,
                is_file,
                is_symlink,
                size_bytes,
                modified,
            });
        }

        let total = entries.len();
        Ok(ListDirResult {
            path: target_dir,
            entries,
            total_entries: total,
            truncated,
        })
    }

    /// Runs a command through TaskManager.
    pub async fn run_command(&self, args: RunCommandArgs) -> Result<RunCommandResult> {
        let wait_ms = args.wait_ms.unwrap_or(5000);
        let cwd = args.cwd.as_deref().or(Some(&self.workspace_dir));
        let is_daemon = args.is_daemon.unwrap_or(false);

        self.task_manager
            .run_command(
                &args.command,
                wait_ms,
                cwd,
                is_daemon,
                args.summary.as_deref(),
            )
            .await
    }

    /// Manages background tasks (list, status, send_input, kill).
    pub async fn manage_task(&self, args: ManageTaskArgs) -> Result<serde_json::Value> {
        match args.action {
            ManageTaskAction::List => {
                let list = self.task_manager.list_tasks().await;
                Ok(serde_json::to_value(list)?)
            }
            ManageTaskAction::Status => {
                let task_id = args
                    .task_id
                    .ok_or_else(|| anyhow!("TaskId is required for 'status' action"))?;
                let status_info = self
                    .task_manager
                    .get_status(&task_id, args.tail_lines)
                    .await?;
                Ok(serde_json::to_value(status_info)?)
            }
            ManageTaskAction::SendInput => {
                let task_id = args
                    .task_id
                    .ok_or_else(|| anyhow!("TaskId is required for 'send_input' action"))?;
                let input = args
                    .input
                    .ok_or_else(|| anyhow!("Input text is required for 'send_input' action"))?;
                self.task_manager.send_input(&task_id, &input).await?;
                Ok(json!({ "status": "input_sent", "task_id": task_id }))
            }
            ManageTaskAction::Kill => {
                let task_id = args
                    .task_id
                    .ok_or_else(|| anyhow!("TaskId is required for 'kill' action"))?;
                self.task_manager.kill_task(&task_id).await?;
                Ok(json!({ "status": "killed", "task_id": task_id }))
            }
            ManageTaskAction::Tail => {
                let task_id = args
                    .task_id
                    .ok_or_else(|| anyhow!("TaskId is required for 'tail' action"))?;
                let lines = self
                    .task_manager
                    .tail_log(&task_id, args.tail_lines.unwrap_or(50))
                    .await?;
                Ok(json!({ "task_id": task_id, "lines": lines }))
            }
        }
    }

    /// Fast file pattern matching using glob patterns (e.g. '**/*.rs', 'src/**/*.ts'). Respects .gitignore.
    pub async fn glob(&self, args: GlobArgs) -> Result<GlobResult> {
        let base_dir = match &args.path {
            Some(p) => self.resolve_path(p)?,
            None => self.workspace_dir.clone(),
        };

        if !base_dir.exists() {
            return Err(anyhow!("Path not found: '{}'", base_dir.display()));
        }

        let glob = globset::GlobBuilder::new(&args.pattern)
            .literal_separator(true)
            .build()
            .or_else(|_| globset::Glob::new(&args.pattern))
            .with_context(|| format!("Invalid glob pattern: '{}'", args.pattern))?;
        let matcher = glob.compile_matcher();

        let max_results = args.max_results.unwrap_or(200);
        let mut matches = Vec::new();
        let mut truncated = false;

        let mut builder = ignore::WalkBuilder::new(&base_dir);
        builder
            .hidden(!args.show_hidden)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false);

        for result in builder.build() {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let rel_path = path.strip_prefix(&base_dir).unwrap_or(path);
            let ws_rel_path = path.strip_prefix(&self.workspace_dir).unwrap_or(path);

            let is_match = matcher.is_match(rel_path)
                || matcher.is_match(entry.file_name())
                || matcher.is_match(ws_rel_path);

            if is_match {
                if matches.len() >= max_results {
                    truncated = true;
                    break;
                }
                matches.push(ws_rel_path.to_path_buf());
            }
        }

        let total = matches.len();
        Ok(GlobResult {
            pattern: args.pattern,
            base_path: base_dir,
            matches,
            total_matches: total,
            truncated,
        })
    }

    /// Fast regex search across files in the workspace. Skips binary files and respects .gitignore.
    pub async fn grep(&self, args: GrepArgs) -> Result<GrepResult> {
        let target_dir = match &args.path {
            Some(p) => self.resolve_path(p)?,
            None => self.workspace_dir.clone(),
        };

        if !target_dir.exists() {
            return Err(anyhow!("Path not found: '{}'", target_dir.display()));
        }

        let re = regex::RegexBuilder::new(&args.pattern)
            .case_insensitive(args.case_insensitive)
            .build()
            .with_context(|| format!("Invalid regex pattern: '{}'", args.pattern))?;

        let glob_matcher = if let Some(g) = &args.glob {
            let compiled = globset::GlobBuilder::new(g)
                .literal_separator(true)
                .build()
                .or_else(|_| globset::Glob::new(g))
                .with_context(|| format!("Invalid file filter glob: '{}'", g))?;
            Some(compiled.compile_matcher())
        } else {
            None
        };

        let max_results = args.max_results.unwrap_or(100);
        let context_lines = args.context_lines.unwrap_or(0);
        let mut matches = Vec::new();
        let mut truncated = false;

        let mut walker_builder = ignore::WalkBuilder::new(&target_dir);
        walker_builder
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false);

        for result in walker_builder.build() {
            let entry = match result {
                Ok(e) => e,
                Err(_) => continue,
            };

            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();

            if let Some(ref gm) = glob_matcher {
                let file_name = entry.file_name();
                let rel = path.strip_prefix(&target_dir).unwrap_or(path);
                if !gm.is_match(file_name) && !gm.is_match(rel) {
                    continue;
                }
            }

            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if BINARY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                    continue;
                }
            }

            let raw_bytes = match tokio::fs::read(path).await {
                Ok(b) => b,
                Err(_) => continue,
            };

            let probe_len = raw_bytes.len().min(512);
            if raw_bytes[..probe_len].contains(&0x00) {
                continue;
            }

            let text = String::from_utf8_lossy(&raw_bytes);
            let lines: Vec<&str> = text.lines().collect();

            let rel_path = path
                .strip_prefix(&self.workspace_dir)
                .unwrap_or(path)
                .to_path_buf();

            for (idx, line) in lines.iter().enumerate() {
                if re.is_match(line) {
                    if matches.len() >= max_results {
                        truncated = true;
                        break;
                    }

                    let line_number = idx + 1;
                    let before_context = if context_lines > 0 {
                        let start = idx.saturating_sub(context_lines);
                        Some(lines[start..idx].iter().map(|s| s.to_string()).collect())
                    } else {
                        None
                    };

                    let after_context = if context_lines > 0 {
                        let end = (idx + 1 + context_lines).min(lines.len());
                        Some(lines[idx + 1..end].iter().map(|s| s.to_string()).collect())
                    } else {
                        None
                    };

                    matches.push(GrepMatch {
                        path: rel_path.clone(),
                        line_number,
                        line_content: line.to_string(),
                        before_context,
                        after_context,
                    });
                }
            }

            if truncated {
                break;
            }
        }

        let total = matches.len();
        Ok(GrepResult {
            pattern: args.pattern,
            matches,
            total_matches: total,
            truncated,
        })
    }

    /// Dispatches dynamic tool execution from JSON-RPC / tool-call payload.
    pub async fn execute_tool(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        match name {
            "read_file" => {
                let params: ReadFileArgs = serde_json::from_value(args)?;
                let res = self.read_file(params).await?;
                Ok(serde_json::to_value(res)?)
            }
            "write_file" => {
                let params: WriteFileArgs = serde_json::from_value(args)?;
                let res = self.write_file(params).await?;
                Ok(serde_json::to_value(res)?)
            }
            "list_directory" => {
                let params: ListDirArgs = serde_json::from_value(args).unwrap_or_default();
                let res = self.list_directory(params).await?;
                Ok(serde_json::to_value(res)?)
            }
            "run_command" => {
                let params: RunCommandArgs = serde_json::from_value(args)?;
                let res = self.run_command(params).await?;
                Ok(serde_json::to_value(res)?)
            }
            "manage_task" => {
                let params: ManageTaskArgs = serde_json::from_value(args)?;
                self.manage_task(params).await
            }
            "glob" => {
                let params: GlobArgs = serde_json::from_value(args)?;
                let res = self.glob(params).await?;
                Ok(serde_json::to_value(res)?)
            }
            "grep" => {
                let params: GrepArgs = serde_json::from_value(args)?;
                let res = self.grep(params).await?;
                Ok(serde_json::to_value(res)?)
            }
            other => Err(anyhow!("Unknown tool name: '{}'", other)),
        }
    }

    /// Returns standard JSON schemas for all native tools supported by the harness.
    pub fn list_tool_definitions(&self) -> Vec<serde_json::Value> {
        vec![
            json!({
                "name": "read_file",
                "description": "Read file contents with bounded line and byte ranges.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to file" },
                        "start_line": { "type": "integer", "description": "1-indexed start line" },
                        "end_line": { "type": "integer", "description": "1-indexed end line (inclusive)" },
                        "byte_offset": { "type": "integer", "description": "Byte offset into file" },
                        "max_bytes": { "type": "integer", "description": "Max bytes to return" }
                    },
                    "required": ["path"]
                }
            }),
            json!({
                "name": "write_file",
                "description": "Write or append content to a file, creating parent directories.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to file" },
                        "content": { "type": "string", "description": "Content to write" },
                        "overwrite": { "type": "boolean", "description": "Overwrite existing file" },
                        "append": { "type": "boolean", "description": "Append to existing file" }
                    },
                    "required": ["path", "content"]
                }
            }),
            json!({
                "name": "list_directory",
                "description": "List directory contents with optional recursion, depth, and hidden file controls.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path" },
                        "recursive": { "type": "boolean", "description": "Recursively list subdirectories" },
                        "depth": { "type": "integer", "description": "Max recursion depth" },
                        "show_hidden": { "type": "boolean", "description": "Include hidden/dot files" },
                        "max_entries": { "type": "integer", "description": "Max entries to return" }
                    }
                }
            }),
            json!({
                "name": "run_command",
                "description": "Execute a shell command with synchronous wait and async background daemon transition.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Command string to run" },
                        "wait_ms": { "type": "integer", "description": "Milliseconds to wait synchronously" },
                        "cwd": { "type": "string", "description": "Working directory" },
                        "is_daemon": { "type": "boolean", "description": "Mark as persistent daemon" },
                        "summary": { "type": "string", "description": "Brief description of command" }
                    },
                    "required": ["command"]
                }
            }),
            json!({
                "name": "manage_task",
                "description": "Inspect or control background tasks (list, status, send_input, kill).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "enum": ["list", "status", "send_input", "kill", "tail"] },
                        "task_id": { "type": "string", "description": "Target task identifier" },
                        "input": { "type": "string", "description": "Input text for send_input" },
                        "tail_lines": { "type": "integer", "description": "Recent output line count" }
                    },
                    "required": ["action"]
                }
            }),
            json!({
                "name": "glob",
                "description": "Fast file pattern matching using glob patterns (e.g. '**/*.rs', 'src/**/*.ts'). Respects .gitignore.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Glob pattern (e.g. '**/*.rs', '*.json')" },
                        "path": { "type": "string", "description": "Base directory to search in (defaults to workspace)" },
                        "max_results": { "type": "integer", "description": "Maximum number of results to return (default 200)" },
                        "show_hidden": { "type": "boolean", "description": "Include hidden/dot files" }
                    },
                    "required": ["pattern"]
                }
            }),
            json!({
                "name": "grep",
                "description": "Fast regex search across files in the workspace. Skips binary files and respects .gitignore.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regex or substring pattern to search for" },
                        "path": { "type": "string", "description": "Directory or file to search in (defaults to workspace)" },
                        "glob": { "type": "string", "description": "Optional glob filter for filenames (e.g. '*.rs', '*.toml')" },
                        "case_insensitive": { "type": "boolean", "description": "Perform case-insensitive matching" },
                        "max_results": { "type": "integer", "description": "Maximum matching lines to return (default 100)" },
                        "context_lines": { "type": "integer", "description": "Number of context lines before and after matches" }
                    },
                    "required": ["pattern"]
                }
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_and_write_file() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tools_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        let test_file = tmp.join("sub").join("hello.txt");

        // 1. Write new file
        let write_res = registry
            .write_file(WriteFileArgs {
                path: test_file.clone(),
                content: "Line 1\nLine 2\nLine 3\nLine 4\nLine 5\n".to_string(),
                overwrite: false,
                append: false,
            })
            .await
            .expect("Failed to write file");

        assert!(write_res.created);
        assert_eq!(write_res.bytes_written, 35);

        // 2. Write again without overwrite or append -> should error
        let err = registry
            .write_file(WriteFileArgs {
                path: test_file.clone(),
                content: "Conflict".to_string(),
                overwrite: false,
                append: false,
            })
            .await;
        assert!(err.is_err());

        // 3. Append to file
        let append_res = registry
            .write_file(WriteFileArgs {
                path: test_file.clone(),
                content: "Line 6\n".to_string(),
                overwrite: false,
                append: true,
            })
            .await
            .expect("Failed to append");
        assert!(append_res.appended);

        // 4. Read bounded lines 2 to 4
        let read_res = registry
            .read_file(ReadFileArgs {
                path: test_file.clone(),
                start_line: Some(2),
                end_line: Some(4),
                byte_offset: None,
                max_bytes: None,
            })
            .await
            .expect("Failed to read");

        assert_eq!(read_res.start_line, 2);
        assert_eq!(read_res.end_line, 4);
        assert_eq!(read_res.content, "Line 2\nLine 3\nLine 4");
        assert!(read_res.truncated);

        // 5. Overwrite file
        let overwrite_res = registry
            .write_file(WriteFileArgs {
                path: test_file.clone(),
                content: "Overwritten content".to_string(),
                overwrite: true,
                append: false,
            })
            .await
            .expect("Failed to overwrite");
        assert!(overwrite_res.overwritten);

        let read_over = registry
            .read_file(ReadFileArgs {
                path: test_file.clone(),
                start_line: None,
                end_line: None,
                byte_offset: None,
                max_bytes: None,
            })
            .await
            .unwrap();
        assert_eq!(read_over.content, "Overwritten content");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_read_file_binary_guard() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tools_bin_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        // 1. Prohibited extension
        let exe_file = tmp.join("malicious.exe");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(&exe_file, b"FAKE_EXE_BYTES").unwrap();

        let err = registry
            .read_file(ReadFileArgs {
                path: exe_file,
                start_line: None,
                end_line: None,
                byte_offset: None,
                max_bytes: None,
            })
            .await;
        assert!(err.is_err());
        assert!(err
            .unwrap_err()
            .to_string()
            .contains("prohibited for extension"));

        // 2. Null byte payload in .txt file
        let null_file = tmp.join("hidden_binary.txt");
        let mut null_bytes = b"Hello world".to_vec();
        null_bytes.push(0x00);
        null_bytes.extend_from_slice(b"remaining payload");
        std::fs::write(&null_file, null_bytes).unwrap();

        let err2 = registry
            .read_file(ReadFileArgs {
                path: null_file,
                start_line: None,
                end_line: None,
                byte_offset: None,
                max_bytes: None,
            })
            .await;
        assert!(err2.is_err());
        assert!(err2
            .unwrap_err()
            .to_string()
            .contains("Binary payload detected"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_list_directory() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tools_listdir_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(tmp.join("a").join("b")).unwrap();
        std::fs::write(tmp.join("file1.txt"), "f1").unwrap();
        std::fs::write(tmp.join("a").join("file2.txt"), "f2").unwrap();
        std::fs::write(tmp.join(".hidden"), "h").unwrap();

        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        // List top level, exclude hidden
        let list1 = registry
            .list_directory(ListDirArgs {
                path: None,
                recursive: false,
                depth: None,
                show_hidden: false,
                max_entries: None,
            })
            .await
            .unwrap();

        let names: Vec<String> = list1.entries.into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"file1.txt".to_string()));
        assert!(names.contains(&"a".to_string()));
        assert!(!names.contains(&".hidden".to_string()));

        // List recursive with hidden
        let list2 = registry
            .list_directory(ListDirArgs {
                path: None,
                recursive: true,
                depth: Some(3),
                show_hidden: true,
                max_entries: None,
            })
            .await
            .unwrap();

        let names2: Vec<String> = list2.entries.into_iter().map(|e| e.name).collect();
        assert!(names2.contains(&".hidden".to_string()));
        assert!(names2.contains(&"file2.txt".to_string()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_tool_registry_json_dispatch() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tools_dispatch_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        let target_file = tmp.join("dispatched.txt");

        // Execute write_file via JSON
        let write_val = registry
            .execute_tool(
                "write_file",
                json!({
                    "path": target_file.to_string_lossy(),
                    "content": "JSON Dispatched Content",
                    "overwrite": true
                }),
            )
            .await
            .expect("Failed JSON write_file");

        assert_eq!(write_val["created"], true);

        // Execute read_file via JSON
        let read_val = registry
            .execute_tool(
                "read_file",
                json!({
                    "path": target_file.to_string_lossy()
                }),
            )
            .await
            .expect("Failed JSON read_file");

        assert_eq!(read_val["content"], "JSON Dispatched Content");

        // Definitions
        let defs = registry.list_tool_definitions();
        assert_eq!(defs.len(), 7);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_glob_tool() {
        let tmp = std::env::temp_dir().join(format!(
            "agendex_glob_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        let src_dir = tmp.join("src").join("nested");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();

        tokio::fs::write(src_dir.join("lib.rs"), "pub fn test() {}\n")
            .await
            .unwrap();
        tokio::fs::write(src_dir.join("main.rs"), "fn main() {}\n")
            .await
            .unwrap();
        tokio::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"test\"\n")
            .await
            .unwrap();
        tokio::fs::write(tmp.join("README.md"), "# Title\n")
            .await
            .unwrap();

        // 1. Glob matching all .rs files
        let rs_res = registry
            .glob(GlobArgs {
                pattern: "**/*.rs".to_string(),
                path: None,
                max_results: None,
                show_hidden: false,
            })
            .await
            .unwrap();
        assert_eq!(rs_res.total_matches, 2);
        assert!(rs_res.matches.iter().any(|p| p.ends_with("lib.rs")));
        assert!(rs_res.matches.iter().any(|p| p.ends_with("main.rs")));

        // 2. Glob matching .toml files
        let toml_res = registry
            .glob(GlobArgs {
                pattern: "*.toml".to_string(),
                path: None,
                max_results: None,
                show_hidden: false,
            })
            .await
            .unwrap();
        assert_eq!(toml_res.total_matches, 1);
        assert!(toml_res.matches[0].ends_with("Cargo.toml"));

        // 3. Glob dispatched via execute_tool
        let exec_val = registry
            .execute_tool("glob", json!({ "pattern": "*.md" }))
            .await
            .unwrap();
        assert_eq!(exec_val["total_matches"], 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_grep_tool() {
        let tmp = std::env::temp_dir().join(format!(
            "agendex_grep_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        let src_dir = tmp.join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();

        let file1 = src_dir.join("app.rs");
        tokio::fs::write(
            &file1,
            "// Line 1\nlet target_var = 42;\n// Line 3\nlet other_var = 100;\n",
        )
        .await
        .unwrap();

        let file2 = tmp.join("README.md");
        tokio::fs::write(
            &file2,
            "# Documentation\nContains TARGET_VAR in uppercase.\n",
        )
        .await
        .unwrap();

        // 1. Exact case grep
        let grep_res = registry
            .grep(GrepArgs {
                pattern: "target_var".to_string(),
                path: None,
                glob: None,
                case_insensitive: false,
                max_results: None,
                context_lines: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(grep_res.total_matches, 1);
        assert_eq!(grep_res.matches[0].line_number, 2);
        assert!(grep_res.matches[0]
            .line_content
            .contains("let target_var = 42;"));
        assert_eq!(
            grep_res.matches[0].before_context.as_deref(),
            Some(&["// Line 1".to_string()][..])
        );
        assert_eq!(
            grep_res.matches[0].after_context.as_deref(),
            Some(&["// Line 3".to_string()][..])
        );

        // 2. Case-insensitive grep across all files
        let grep_ci = registry
            .grep(GrepArgs {
                pattern: "target_var".to_string(),
                path: None,
                glob: None,
                case_insensitive: true,
                max_results: None,
                context_lines: None,
            })
            .await
            .unwrap();
        assert_eq!(grep_ci.total_matches, 2);

        // 3. Glob filtered grep
        let grep_filtered = registry
            .grep(GrepArgs {
                pattern: "target_var".to_string(),
                path: None,
                glob: Some("*.rs".to_string()),
                case_insensitive: true,
                max_results: None,
                context_lines: None,
            })
            .await
            .unwrap();
        assert_eq!(grep_filtered.total_matches, 1);
        assert!(grep_filtered.matches[0].path.ends_with("app.rs"));

        // 4. Executed via JSON tool dispatch
        let json_exec = registry
            .execute_tool(
                "grep",
                json!({
                    "pattern": "TARGET_VAR",
                    "case_insensitive": false
                }),
            )
            .await
            .unwrap();
        assert_eq!(json_exec["total_matches"], 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_path_traversal_and_workspace_containment() {
        let tmp = std::env::temp_dir().join(format!(
            "agendex_traversal_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let ws_dir = tmp.join("workspace");
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();

        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(ws_dir.clone(), task_manager);

        // Outside file
        let outside_file = tmp.join("secret.txt");
        tokio::fs::write(&outside_file, "classified").await.unwrap();

        // 1. resolve_path rejects relative parent escape
        let rel_escape = registry.resolve_path(Path::new("../secret.txt"));
        assert!(rel_escape.is_err());

        // 2. resolve_path rejects absolute outside path
        let abs_escape = registry.resolve_path(&outside_file);
        assert!(abs_escape.is_err());

        // 3. read_file rejects relative escape
        let read_res = registry
            .read_file(ReadFileArgs {
                path: PathBuf::from("../secret.txt"),
                start_line: None,
                end_line: None,
                byte_offset: None,
                max_bytes: None,
            })
            .await;
        assert!(read_res.is_err());

        // 4. write_file rejects relative escape
        let write_res = registry
            .write_file(WriteFileArgs {
                path: PathBuf::from("../escape.txt"),
                content: "malicious".to_string(),
                overwrite: true,
                append: false,
            })
            .await;
        assert!(write_res.is_err());

        // 5. Valid file inside workspace succeeds
        let valid_write = registry
            .write_file(WriteFileArgs {
                path: PathBuf::from("valid.txt"),
                content: "safe".to_string(),
                overwrite: true,
                append: false,
            })
            .await;
        assert!(valid_write.is_ok());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_grep_and_glob_respects_gitignore_without_git_repo() {
        let tmp = std::env::temp_dir().join(format!(
            "agendex_ignore_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let task_manager = Arc::new(TaskManager::new(tmp.join("tasks")));
        let registry = ToolRegistry::new(tmp.clone(), task_manager);

        // Intentionally DO NOT initialize a git repo here!
        tokio::fs::write(tmp.join(".gitignore"), "target/\n*.log\nnode_modules/\n")
            .await
            .unwrap();

        let src_dir = tmp.join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        tokio::fs::write(src_dir.join("main.rs"), "fn main() { let x = 42; }\n")
            .await
            .unwrap();

        let target_dir = tmp.join("target").join("debug");
        tokio::fs::create_dir_all(&target_dir).await.unwrap();
        tokio::fs::write(target_dir.join("build.rs"), "let x = 42;\n")
            .await
            .unwrap();

        tokio::fs::write(tmp.join("debug.log"), "let x = 42;\n")
            .await
            .unwrap();

        // Grep for 'x = 42' should ONLY find src/main.rs, ignoring target/ and debug.log
        let grep_res = registry
            .grep(GrepArgs {
                pattern: "x = 42".to_string(),
                path: None,
                glob: None,
                case_insensitive: false,
                max_results: None,
                context_lines: None,
            })
            .await
            .unwrap();

        assert_eq!(grep_res.total_matches, 1);
        assert!(grep_res.matches[0].path.ends_with("main.rs"));

        // Glob for '**/*.rs' should ONLY match src/main.rs, ignoring target/
        let glob_res = registry
            .glob(GlobArgs {
                pattern: "**/*.rs".to_string(),
                path: None,
                max_results: None,
                show_hidden: false,
            })
            .await
            .unwrap();

        assert_eq!(glob_res.total_matches, 1);
        assert!(glob_res.matches[0].ends_with("main.rs"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
