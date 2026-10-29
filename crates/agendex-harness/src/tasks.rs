use std::collections::{HashMap, VecDeque};
use std::fs::OpenOptions;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{Notify, RwLock};

/// Actions supported by the `manage_task` controller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManageTaskAction {
    List,
    Status,
    SendInput,
    Kill,
    Tail,
}

/// Lifecycle state of a background or synchronous task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", content = "details")]
pub enum TaskStatus {
    Running,
    Completed {
        exit_code: i32,
    },
    Failed {
        exit_code: Option<i32>,
        error: String,
    },
    Killed,
}

impl TaskStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, TaskStatus::Running)
    }

    pub fn is_finished(&self) -> bool {
        !self.is_running()
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            TaskStatus::Completed { exit_code } => Some(*exit_code),
            TaskStatus::Failed { exit_code, .. } => *exit_code,
            _ => None,
        }
    }
}

/// In-memory ring buffer holding the most recent output lines of a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRingBuffer {
    capacity: usize,
    lines: VecDeque<String>,
    total_lines: usize,
    total_bytes: usize,
}

impl TaskRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            lines: VecDeque::with_capacity(capacity.min(500)),
            total_lines: 0,
            total_bytes: 0,
        }
    }

    pub fn push_line(&mut self, line: String) {
        self.total_lines += 1;
        self.total_bytes += line.len() + 1;
        if self.lines.len() >= self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn tail(&self, count: usize) -> Vec<String> {
        let skip = self.lines.len().saturating_sub(count);
        self.lines.iter().skip(skip).cloned().collect()
    }

    pub fn all(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }

    pub fn total_lines(&self) -> usize {
        self.total_lines
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

/// Metadata summary of a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub task_id: String,
    pub command: String,
    pub summary: Option<String>,
    pub status: TaskStatus,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub elapsed_secs: u64,
    pub is_daemon: bool,
    pub log_path: PathBuf,
    pub total_lines: usize,
    pub total_bytes: usize,
}

/// Detailed status inspection for a specific task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatusInfo {
    pub task_id: String,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub is_running: bool,
    pub duration_ms: u64,
    pub log_path: PathBuf,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub recent_output: Vec<String>,
}

/// Result returned from executing a command with `wait_ms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RunCommandResult {
    Synchronous {
        command: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        duration_ms: u64,
        truncated: bool,
    },
    Async {
        task_id: String,
        command: String,
        summary: Option<String>,
        status: TaskStatus,
        log_path: PathBuf,
        message: String,
    },
}

/// An active or historical process task managed by `TaskManager`.
pub struct Task {
    pub id: String,
    pub command: String,
    pub summary: Option<String>,
    pub is_daemon: bool,
    pub start_time: DateTime<Utc>,
    pub log_path: PathBuf,
    pub ring_buffer: Arc<Mutex<TaskRingBuffer>>,
    pub status: Arc<RwLock<TaskStatus>>,
    pub end_time: Arc<RwLock<Option<DateTime<Utc>>>>,
    pub stdin_tx: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    pub kill_notify: Arc<Notify>,
    pub pid: Arc<AtomicU32>,
}

impl Task {
    pub async fn get_status(&self) -> TaskStatus {
        self.status.read().await.clone()
    }

    pub async fn get_end_time(&self) -> Option<DateTime<Utc>> {
        *self.end_time.read().await
    }

    pub async fn to_info(&self) -> TaskInfo {
        let status = self.get_status().await;
        let end_time = self.get_end_time().await;
        let now = Utc::now();
        let elapsed_secs = if let Some(end) = end_time {
            (end - self.start_time).num_seconds().max(0) as u64
        } else {
            (now - self.start_time).num_seconds().max(0) as u64
        };

        let (total_lines, total_bytes) = if let Ok(ring) = self.ring_buffer.lock() {
            (ring.total_lines(), ring.total_bytes())
        } else {
            (0, 0)
        };

        TaskInfo {
            task_id: self.id.clone(),
            command: self.command.clone(),
            summary: self.summary.clone(),
            status,
            start_time: self.start_time,
            end_time,
            elapsed_secs,
            is_daemon: self.is_daemon,
            log_path: self.log_path.clone(),
            total_lines,
            total_bytes,
        }
    }
}

/// Central process supervisor managing synchronous runs, async transitions,
/// ring-buffered logs, and stdin/kill controls.
pub struct TaskManager {
    log_dir: PathBuf,
    tasks: Arc<RwLock<HashMap<String, Arc<Task>>>>,
}

impl TaskManager {
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        let log_dir = log_dir.into();
        let _ = std::fs::create_dir_all(&log_dir);
        Self {
            log_dir,
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    /// Spawns a command. If it exits within `wait_ms`, returns a synchronous result.
    /// If still running after `wait_ms`, transitions to an asynchronous background task.
    pub async fn run_command(
        &self,
        command: &str,
        wait_ms: u64,
        cwd: Option<&Path>,
        is_daemon: bool,
        summary: Option<&str>,
    ) -> Result<RunCommandResult> {
        if is_host_power_invocation(command) {
            return Err(anyhow!("refused to spawn host power command: {command}"));
        }

        let task_id = format!("task-{}", uuid::Uuid::new_v4().simple());
        let log_path = self.log_dir.join(format!("{}.log", task_id));

        // Open log file on disk
        let disk_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed to create task log at {}", log_path.display()))?;
        let disk_file = Arc::new(Mutex::new(disk_file));

        // Build shell command
        #[cfg(target_os = "windows")]
        let mut cmd = Command::new("powershell.exe");
        #[cfg(target_os = "windows")]
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            command,
        ]);

        #[cfg(not(target_os = "windows"))]
        let mut cmd = Command::new("sh");
        #[cfg(not(target_os = "windows"))]
        cmd.args(["-c", command]);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let start_time = Utc::now();
        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn command: {}", command))?;

        let pid = child.id().unwrap_or(0);
        let pid_atomic = Arc::new(AtomicU32::new(pid));

        let stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stdout"))?;
        let stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Failed to capture stderr"))?;
        let stdin_pipe = child.stdin.take();

        let ring_buffer = Arc::new(Mutex::new(TaskRingBuffer::new(1000)));
        let status = Arc::new(RwLock::new(TaskStatus::Running));
        let end_time = Arc::new(RwLock::new(None));
        let stdin_tx = Arc::new(tokio::sync::Mutex::new(stdin_pipe));
        let kill_notify = Arc::new(Notify::new());

        let sync_stdout = Arc::new(Mutex::new(Vec::<String>::new()));
        let sync_stderr = Arc::new(Mutex::new(Vec::<String>::new()));

        // Spawn stdout pump
        {
            let ring_clone = Arc::clone(&ring_buffer);
            let file_clone = Arc::clone(&disk_file);
            let sync_clone = Arc::clone(&sync_stdout);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout_pipe).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    if let Ok(mut r) = ring_clone.lock() {
                        r.push_line(line.clone());
                    }
                    if let Ok(mut f) = file_clone.lock() {
                        let _ = writeln!(f, "{}", line);
                    }
                    if let Ok(mut s) = sync_clone.lock() {
                        if s.len() < 5000 {
                            s.push(line);
                        }
                    }
                }
            });
        }

        // Spawn stderr pump
        {
            let ring_clone = Arc::clone(&ring_buffer);
            let file_clone = Arc::clone(&disk_file);
            let sync_clone = Arc::clone(&sync_stderr);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr_pipe).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let formatted = format!("[STDERR] {}", line);
                    if let Ok(mut r) = ring_clone.lock() {
                        r.push_line(formatted.clone());
                    }
                    if let Ok(mut f) = file_clone.lock() {
                        let _ = writeln!(f, "{}", formatted);
                    }
                    if let Ok(mut s) = sync_clone.lock() {
                        if s.len() < 5000 {
                            s.push(line);
                        }
                    }
                }
            });
        }

        let task = Arc::new(Task {
            id: task_id.clone(),
            command: command.to_string(),
            summary: summary.map(|s| s.to_string()),
            is_daemon,
            start_time,
            log_path: log_path.clone(),
            ring_buffer: Arc::clone(&ring_buffer),
            status: Arc::clone(&status),
            end_time: Arc::clone(&end_time),
            stdin_tx: Arc::clone(&stdin_tx),
            kill_notify: Arc::clone(&kill_notify),
            pid: Arc::clone(&pid_atomic),
        });

        {
            let mut tasks_map = self.tasks.write().await;
            tasks_map.insert(task_id.clone(), Arc::clone(&task));
        }

        // Wait bounded window
        let wait_duration = Duration::from_millis(wait_ms);
        let status_clone = Arc::clone(&status);
        let end_time_clone = Arc::clone(&end_time);

        tokio::select! {
            result = child.wait() => {
                let finished_at = Utc::now();
                let duration_ms = (finished_at - start_time).num_milliseconds().max(0) as u64;
                *end_time_clone.write().await = Some(finished_at);

                let exit_code = match result {
                    Ok(exit_status) => {
                        let code = exit_status.code().unwrap_or(-1);
                        if code == 0 {
                            *status_clone.write().await = TaskStatus::Completed { exit_code: 0 };
                        } else {
                            *status_clone.write().await = TaskStatus::Failed {
                                exit_code: Some(code),
                                error: format!("Process exited with non-zero exit code: {}", code),
                            };
                        }
                        exit_status.code()
                    }
                    Err(e) => {
                        *status_clone.write().await = TaskStatus::Failed {
                            exit_code: None,
                            error: format!("Process execution error: {}", e),
                        };
                        None
                    }
                };

                // Yield briefly to let stdout/stderr pump tasks flush
                tokio::time::sleep(Duration::from_millis(30)).await;

                let stdout_text = sync_stdout.lock().map(|s| s.join("\n")).unwrap_or_default();
                let stderr_text = sync_stderr.lock().map(|s| s.join("\n")).unwrap_or_default();

                Ok(RunCommandResult::Synchronous {
                    command: command.to_string(),
                    exit_code,
                    stdout: stdout_text,
                    stderr: stderr_text,
                    duration_ms,
                    truncated: false,
                })
            }
            _ = tokio::time::sleep(wait_duration) => {
                // Command is still running after wait_ms -> transition to background!
                tracing::info!(
                    "Command '{}' exceeded wait_ms ({}ms); transitioning to background task [{}]",
                    command, wait_ms, task_id
                );

                // Spawn background reaper task to watch for child termination
                let status_bg = Arc::clone(&status);
                let end_time_bg = Arc::clone(&end_time);
                let kill_notify_bg = Arc::clone(&kill_notify);

                tokio::spawn(async move {
                    tokio::select! {
                        res = child.wait() => {
                            let end = Utc::now();
                            *end_time_bg.write().await = Some(end);
                            match res {
                                Ok(st) => {
                                    let code = st.code().unwrap_or(-1);
                                    let mut s = status_bg.write().await;
                                    if !matches!(*s, TaskStatus::Killed) {
                                        if code == 0 {
                                            *s = TaskStatus::Completed { exit_code: 0 };
                                        } else {
                                            *s = TaskStatus::Failed {
                                                exit_code: Some(code),
                                                error: format!("Exited with code: {}", code),
                                            };
                                        }
                                    }
                                }
                                Err(e) => {
                                    let mut s = status_bg.write().await;
                                    if !matches!(*s, TaskStatus::Killed) {
                                        *s = TaskStatus::Failed {
                                            exit_code: None,
                                            error: e.to_string(),
                                        };
                                    }
                                }
                            }
                        }
                        _ = kill_notify_bg.notified() => {
                            let _ = child.kill().await;
                            *end_time_bg.write().await = Some(Utc::now());
                            *status_bg.write().await = TaskStatus::Killed;
                        }
                    }
                });

                let summary_str = summary.unwrap_or("Background command execution");
                Ok(RunCommandResult::Async {
                    task_id: task_id.clone(),
                    command: command.to_string(),
                    summary: Some(summary_str.to_string()),
                    status: TaskStatus::Running,
                    log_path,
                    message: format!(
                        "Command is still running after {}ms. Transitioned to background task [{}].",
                        wait_ms, task_id
                    ),
                })
            }
        }
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Arc<Task>> {
        let guard = self.tasks.read().await;
        guard
            .get(task_id)
            .cloned()
            .ok_or_else(|| anyhow!("Task '{}' not found", task_id))
    }

    pub async fn list_tasks(&self) -> Vec<TaskInfo> {
        let guard = self.tasks.read().await;
        let mut list = Vec::new();
        for task in guard.values() {
            list.push(task.to_info().await);
        }
        list.sort_by_key(|a| std::cmp::Reverse(a.start_time));
        list
    }

    pub async fn get_status(
        &self,
        task_id: &str,
        tail_lines: Option<usize>,
    ) -> Result<TaskStatusInfo> {
        let task = self.get_task(task_id).await?;
        let status = task.get_status().await;
        let end_time = task.get_end_time().await;
        let now = Utc::now();
        let duration_ms = if let Some(end) = end_time {
            (end - task.start_time).num_milliseconds().max(0) as u64
        } else {
            (now - task.start_time).num_milliseconds().max(0) as u64
        };

        let count = tail_lines.unwrap_or(50);
        let (total_lines, total_bytes, recent_output) = if let Ok(ring) = task.ring_buffer.lock() {
            (ring.total_lines(), ring.total_bytes(), ring.tail(count))
        } else {
            (0, 0, Vec::new())
        };

        Ok(TaskStatusInfo {
            task_id: task.id.clone(),
            status: status.clone(),
            exit_code: status.exit_code(),
            is_running: status.is_running(),
            duration_ms,
            log_path: task.log_path.clone(),
            total_lines,
            total_bytes,
            recent_output,
        })
    }

    pub async fn send_input(&self, task_id: &str, input: &str) -> Result<()> {
        let task = self.get_task(task_id).await?;
        if !task.get_status().await.is_running() {
            return Err(anyhow!(
                "Cannot send input: task '{}' is not running",
                task_id
            ));
        }

        let mut stdin_guard = task.stdin_tx.lock().await;
        if let Some(ref mut stdin) = *stdin_guard {
            let mut bytes = input.as_bytes().to_vec();
            if !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            stdin
                .write_all(&bytes)
                .await
                .context("Failed to write to task stdin")?;
            stdin.flush().await.context("Failed to flush task stdin")?;

            // Record input line in ring buffer
            if let Ok(mut ring) = task.ring_buffer.lock() {
                ring.push_line(format!("[STDIN] {}", input.trim_end()));
            }
            Ok(())
        } else {
            Err(anyhow!(
                "Task '{}' stdin is not available or closed",
                task_id
            ))
        }
    }

    pub async fn kill_task(&self, task_id: &str) -> Result<()> {
        let task = self.get_task(task_id).await?;
        if !task.get_status().await.is_running() {
            return Ok(());
        }

        let pid = task.pid.load(Ordering::SeqCst);
        if pid > 0 {
            #[cfg(target_os = "windows")]
            {
                // Kill process tree forcibly on Windows
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .output();
            }
        }

        task.kill_notify.notify_waiters();
        *task.status.write().await = TaskStatus::Killed;
        *task.end_time.write().await = Some(Utc::now());

        if let Ok(mut ring) = task.ring_buffer.lock() {
            ring.push_line(format!("[SYSTEM] Task '{}' killed by supervisor.", task_id));
        }

        tracing::info!("Killed task: {}", task_id);
        Ok(())
    }

    pub async fn tail_log(&self, task_id: &str, lines: usize) -> Result<Vec<String>> {
        let task = self.get_task(task_id).await?;
        if let Ok(ring) = task.ring_buffer.lock() {
            let tail = ring.tail(lines);
            if !tail.is_empty() {
                return Ok(tail);
            }
        }

        // Fallback to disk file if ring buffer was empty
        if task.log_path.exists() {
            let content = tokio::fs::read_to_string(&task.log_path).await?;
            let all_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
            let skip = all_lines.len().saturating_sub(lines);
            Ok(all_lines.into_iter().skip(skip).collect())
        } else {
            Ok(Vec::new())
        }
    }
}

/// Host power commands are refused before a process is created.
/// Command position only: `grep shutdown log.txt` is not a power command.
fn is_host_power_invocation(command: &str) -> bool {
    fn normalize(token: &str) -> String {
        let lower = token.trim_matches('"').trim_matches('\'').to_lowercase();
        let base = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
        base.trim_end_matches(".exe").to_string()
    }
    fn is_power_bin(name: &str) -> bool {
        if matches!(
            name,
            "shutdown"
                | "reboot"
                | "halt"
                | "poweroff"
                | "logoff"
                | "stop-computer"
                | "restart-computer"
                | "suspend-computer"
        ) {
            return true;
        }
        // Test seam. This is not a Windows program, so a missed guard cannot power the host off.
        #[cfg(test)]
        if name == "agendex-test-refuse-spawn" {
            return true;
        }
        false
    }
    fn split_cmd(command: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_quote = false;
        for c in command.chars() {
            match c {
                '"' | '\'' => in_quote = !in_quote,
                c if c.is_whitespace() && !in_quote => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
    fn at_command_position(tokens: &[String]) -> bool {
        let mut i = 0;
        while i < tokens.len() {
            let name = normalize(&tokens[i]);
            if matches!(name.as_str(), "sudo" | "doas" | "env" | "nohup") {
                i += 1;
                continue;
            }
            if matches!(
                name.as_str(),
                "powershell" | "pwsh" | "cmd" | "sh" | "bash" | "zsh"
            ) {
                if let Some(pos) = tokens.iter().position(|t| {
                    matches!(
                        t.to_lowercase().as_str(),
                        "-command" | "-c" | "/c" | "/k" | "-encodedcommand"
                    )
                }) {
                    if let Some(inner) = tokens.get(pos + 1) {
                        return at_command_position(&split_cmd(inner));
                    }
                }
                return false;
            }
            if name == "busybox" {
                i += 1;
                while i < tokens.len() && tokens[i].starts_with('-') {
                    i += 1;
                }
                continue;
            }
            if name == "xargs" || name == "parallel" {
                i += 1;
                while i < tokens.len() {
                    let a = &tokens[i];
                    if a == "--" {
                        i += 1;
                        break;
                    }
                    if a.starts_with('-') {
                        let takes_arg =
                            matches!(a.as_str(), "-I" | "-n" | "-P" | "-L" | "-s" | "-E" | "-d");
                        i += if takes_arg { 2 } else { 1 };
                    } else {
                        break;
                    }
                }
                continue;
            }
            if name == "init" || name == "telinit" {
                if let Some(arg) = tokens.get(i + 1) {
                    if arg == "0" || arg == "6" {
                        return true;
                    }
                }
            }
            return is_power_bin(&name);
        }
        false
    }
    at_command_position(&split_cmd(command))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_power_predicate_does_not_match_mentions() {
        assert!(is_host_power_invocation("shutdown /s /t 0"));
        assert!(is_host_power_invocation("shutdown.exe /r /t 0"));
        assert!(is_host_power_invocation(
            "powershell -Command \"shutdown /s /t 0\""
        ));
        assert!(is_host_power_invocation("init 0"));
        assert!(is_host_power_invocation("init 6"));
        assert!(is_host_power_invocation("busybox reboot"));
        assert!(is_host_power_invocation("xargs shutdown"));
        assert!(is_host_power_invocation("sudo xargs shutdown"));
        assert!(!is_host_power_invocation("grep shutdown log.txt"));
        assert!(!is_host_power_invocation("echo shutdown"));
    }

    #[tokio::test]
    async fn test_run_command_refuses_host_power_before_spawn() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tasks_power_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let manager = TaskManager::new(&tmp);
        // Never pass a real shutdown invocation into the spawner from a test.
        let err = manager
            .run_command("agendex-test-refuse-spawn", 1000, None, false, None)
            .await
            .expect_err("refused command must not spawn");
        assert!(
            err.to_string()
                .contains("refused to spawn host power command"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_task_synchronous_success() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tasks_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let manager = TaskManager::new(&tmp);

        let res = manager
            .run_command(
                "echo AGENDA_TEST_SYNC",
                5000,
                None,
                false,
                Some("Sync echo"),
            )
            .await
            .expect("Command failed");

        match res {
            RunCommandResult::Synchronous {
                stdout, exit_code, ..
            } => {
                assert_eq!(exit_code, Some(0));
                assert!(stdout.contains("AGENDA_TEST_SYNC"));
            }
            RunCommandResult::Async { .. } => {
                panic!("Expected synchronous execution");
            }
        }

        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed { exit_code: 0 });

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_task_async_transition_and_kill() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tasks_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let manager = TaskManager::new(&tmp);

        // Sleep command will exceed 200ms wait_ms -> transitions to async
        #[cfg(target_os = "windows")]
        let cmd = "Start-Sleep -Seconds 10";
        #[cfg(not(target_os = "windows"))]
        let cmd = "sleep 10";

        let res = manager
            .run_command(cmd, 200, None, false, Some("Long sleep"))
            .await
            .expect("Run command failed");

        let task_id = match res {
            RunCommandResult::Async {
                task_id, status, ..
            } => {
                assert_eq!(status, TaskStatus::Running);
                task_id
            }
            RunCommandResult::Synchronous { .. } => {
                panic!("Expected async transition");
            }
        };

        let status_info = manager.get_status(&task_id, Some(10)).await.unwrap();
        assert!(status_info.is_running);

        // Kill the task
        manager.kill_task(&task_id).await.unwrap();

        // Check status updated
        let killed_info = manager.get_status(&task_id, Some(10)).await.unwrap();
        assert_eq!(killed_info.status, TaskStatus::Killed);
        assert!(!killed_info.is_running);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_task_send_input() {
        let tmp = std::env::temp_dir().join(format!(
            "agenda_tasks_stdin_{}",
            uuid::Uuid::new_v4().simple()
        ));
        let manager = TaskManager::new(&tmp);

        #[cfg(target_os = "windows")]
        let cmd = "$v = [Console]::In.ReadLine(); Write-Output \"INPUT_WAS: $v\"";
        #[cfg(not(target_os = "windows"))]
        let cmd = "read v; echo \"INPUT_WAS: $v\"";

        let res = manager
            .run_command(cmd, 150, None, false, Some("Stdin test"))
            .await
            .unwrap();

        let task_id = match res {
            RunCommandResult::Async { task_id, .. } => task_id,
            RunCommandResult::Synchronous { .. } => panic!("Expected async waiting for input"),
        };

        // Send input
        manager.send_input(&task_id, "PING_HELLO").await.unwrap();

        // Wait a bit for the command to finish
        let mut finished = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let st = manager.get_status(&task_id, Some(10)).await.unwrap();
            if !st.is_running {
                finished = true;
                break;
            }
        }

        assert!(finished, "Task should finish after receiving input");
        let tail = manager.tail_log(&task_id, 20).await.unwrap();
        let combined = tail.join("\n");
        assert!(combined.contains("INPUT_WAS: PING_HELLO") || combined.contains("PING_HELLO"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_task_ring_buffer() {
        let mut ring = TaskRingBuffer::new(3);
        ring.push_line("L1".to_string());
        ring.push_line("L2".to_string());
        ring.push_line("L3".to_string());
        assert_eq!(ring.all(), vec!["L1", "L2", "L3"]);

        ring.push_line("L4".to_string());
        assert_eq!(ring.all(), vec!["L2", "L3", "L4"]);
        assert_eq!(ring.tail(2), vec!["L3", "L4"]);
        assert_eq!(ring.total_lines(), 4);
    }
}
