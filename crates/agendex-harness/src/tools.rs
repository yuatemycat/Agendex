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

// ============================================================================
// ToolRegistry
// ============================================================================

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

    /// Resolves a path safely against workspace directory.
    pub fn resolve_path(&self, p: &Path) -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace_dir.join(p)
        }
    }

    /// Reads a file with line-bound and byte-bound safeguards and binary rejection.
    pub async fn read_file(&self, args: ReadFileArgs) -> Result<ReadFileResult> {
        let resolved = self.resolve_path(&args.path);
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
        let resolved = self.resolve_path(&args.path);

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
            Some(ref p) => self.resolve_path(p),
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
        assert_eq!(defs.len(), 5);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
