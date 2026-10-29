use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Lifecycle points where hooks can intercept and validate agent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookPoint {
    PreTurn,
    PostTool,
    WorkspaceChange,
    Stop,
}

impl HookPoint {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreTurn => "pre-turn",
            Self::PostTool => "post-tool",
            Self::WorkspaceChange => "workspace-change",
            Self::Stop => "stop",
        }
    }
}

/// Payload delivered to a hook when triggered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookEvent {
    PreTurn {
        user_input: String,
    },
    PostTool {
        tool_name: String,
        tool_result: String,
    },
    WorkspaceChange {
        old_path: PathBuf,
        new_path: PathBuf,
    },
    Stop {
        reason: String,
    },
}

impl HookEvent {
    pub fn point(&self) -> HookPoint {
        match self {
            Self::PreTurn { .. } => HookPoint::PreTurn,
            Self::PostTool { .. } => HookPoint::PostTool,
            Self::WorkspaceChange { .. } => HookPoint::WorkspaceChange,
            Self::Stop { .. } => HookPoint::Stop,
        }
    }
}

/// Outcome of a single hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookExecution {
    pub hook_name: String,
    pub point: HookPoint,
    pub success: bool,
    pub output: String,
    pub execution_time_ms: u64,
}

/// Aggregate summary of all hooks executed during a single lifecycle trigger.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookSummary {
    pub executions: Vec<HookExecution>,
    pub all_succeeded: bool,
}

/// Asynchronous trait for programmatic hook handlers.
#[async_trait::async_trait]
pub trait HookHandler: Send + Sync {
    async fn execute(&self, event: &HookEvent) -> anyhow::Result<String>;
}

/// Closure wrapper implementing `HookHandler`.
pub struct FnHookHandler<F> {
    func: F,
}

impl<F> FnHookHandler<F> {
    pub fn new(func: F) -> Self {
        Self { func }
    }
}

#[async_trait::async_trait]
impl<F, Fut> HookHandler for FnHookHandler<F>
where
    F: Fn(HookEvent) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = anyhow::Result<String>> + Send,
{
    async fn execute(&self, event: &HookEvent) -> anyhow::Result<String> {
        (self.func)(event.clone()).await
    }
}

type HookList = Vec<(String, Arc<dyn HookHandler>)>;
type ProgrammaticHookMap = Arc<RwLock<HashMap<HookPoint, HookList>>>;

/// Deterministic lifecycle hooks supervisor and executor.
pub struct HookDispatcher {
    workspace_dir: PathBuf,
    hooks_dir: PathBuf,
    programmatic_hooks: ProgrammaticHookMap,
    timeout: Duration,
}

impl HookDispatcher {
    /// Initializes a new HookDispatcher for the specified workspace.
    pub async fn new(workspace_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let ws = workspace_dir.as_ref().to_path_buf();
        let hooks_dir = if ws.join(".agendex").join("hooks").exists() {
            ws.join(".agendex").join("hooks")
        } else {
            ws.join(".agenda").join("hooks")
        };

        Ok(Self {
            workspace_dir: ws,
            hooks_dir,
            programmatic_hooks: Arc::new(RwLock::new(HashMap::new())),
            timeout: Duration::from_secs(5),
        })
    }

    /// Sets the maximum execution duration allowed for any single hook script.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Registers a programmatic hook handler for a specific lifecycle point.
    pub async fn register_hook<H: HookHandler + 'static>(
        &self,
        point: HookPoint,
        name: impl Into<String>,
        handler: H,
    ) {
        let mut map = self.programmatic_hooks.write().await;
        map.entry(point)
            .or_default()
            .push((name.into(), Arc::new(handler)));
    }

    /// Registers a programmatic async closure hook handler.
    pub async fn register_fn<F, Fut>(&self, point: HookPoint, name: impl Into<String>, f: F)
    where
        F: Fn(HookEvent) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        self.register_hook(point, name, FnHookHandler::new(f)).await;
    }

    /// Triggers `pre-turn` hooks before planner prompt execution.
    pub async fn trigger_pre_turn(&self, user_input: &str) -> anyhow::Result<HookSummary> {
        self.dispatch(HookEvent::PreTurn {
            user_input: user_input.to_string(),
        })
        .await
    }

    /// Triggers `post-tool` hooks after a tool execution completes.
    pub async fn trigger_post_tool(
        &self,
        tool_name: &str,
        tool_result: &str,
    ) -> anyhow::Result<HookSummary> {
        self.dispatch(HookEvent::PostTool {
            tool_name: tool_name.to_string(),
            tool_result: tool_result.to_string(),
        })
        .await
    }

    /// Triggers `workspace-change` hooks when the active repository path shifts.
    pub async fn trigger_workspace_change(
        &self,
        old_path: &Path,
        new_path: &Path,
    ) -> anyhow::Result<HookSummary> {
        self.dispatch(HookEvent::WorkspaceChange {
            old_path: old_path.to_path_buf(),
            new_path: new_path.to_path_buf(),
        })
        .await
    }

    /// Triggers `stop` hooks when the agent session concludes or aborts.
    pub async fn trigger_stop(&self, reason: &str) -> anyhow::Result<HookSummary> {
        self.dispatch(HookEvent::Stop {
            reason: reason.to_string(),
        })
        .await
    }

    /// Dispatches a lifecycle hook event to both programmatic handlers and filesystem scripts.
    pub async fn dispatch(&self, event: HookEvent) -> anyhow::Result<HookSummary> {
        let point = event.point();
        let mut executions = Vec::new();

        // 1. Execute programmatic hooks
        let handlers = {
            let map = self.programmatic_hooks.read().await;
            map.get(&point).cloned().unwrap_or_default()
        };

        for (name, handler) in handlers {
            let start = Instant::now();
            let result = handler.execute(&event).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(output) => {
                    executions.push(HookExecution {
                        hook_name: name,
                        point,
                        success: true,
                        output,
                        execution_time_ms: elapsed_ms,
                    });
                }
                Err(e) => {
                    tracing::warn!("Programmatic hook '{}' failed: {}", name, e);
                    executions.push(HookExecution {
                        hook_name: name,
                        point,
                        success: false,
                        output: e.to_string(),
                        execution_time_ms: elapsed_ms,
                    });
                }
            }
        }

        // 2. Discover and execute filesystem script hooks in .agenda/hooks/
        let scripts = self.find_script_hooks(point).await;
        for script_path in scripts {
            let script_name = script_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown_hook")
                .to_string();

            let start = Instant::now();
            let result = self.execute_script_hook(&script_path, &event).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(output) => {
                    executions.push(HookExecution {
                        hook_name: script_name,
                        point,
                        success: true,
                        output,
                        execution_time_ms: elapsed_ms,
                    });
                }
                Err(e) => {
                    tracing::warn!("Script hook '{}' failed: {}", script_name, e);
                    executions.push(HookExecution {
                        hook_name: script_name,
                        point,
                        success: false,
                        output: e.to_string(),
                        execution_time_ms: elapsed_ms,
                    });
                }
            }
        }

        let all_succeeded = executions.iter().all(|e| e.success);
        Ok(HookSummary {
            executions,
            all_succeeded,
        })
    }

    /// Discovers script files matching the lifecycle point in `.agenda/hooks/`.
    async fn find_script_hooks(&self, point: HookPoint) -> Vec<PathBuf> {
        let mut matched = Vec::new();
        if !self.hooks_dir.is_dir() {
            return matched;
        }

        let point_name = point.as_str();

        // Check for direct files: <point>.bat, <point>.cmd, <point>.ps1, <point>.sh, <point>.exe
        let extensions = ["bat", "cmd", "ps1", "sh", "exe"];
        for ext in extensions {
            let candidate = self.hooks_dir.join(format!("{}.{}", point_name, ext));
            if candidate.is_file() {
                matched.push(candidate);
            }
        }

        // Check for a sub-directory: .agenda/hooks/<point>/*
        let sub_dir = self.hooks_dir.join(point_name);
        if sub_dir.is_dir() {
            if let Ok(mut entries) = tokio::fs::read_dir(sub_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.is_file() {
                        matched.push(path);
                    }
                }
            }
        }

        matched
    }

    /// Executes a script hook with sandboxed environment variables and timeout guard.
    async fn execute_script_hook(
        &self,
        script: &Path,
        event: &HookEvent,
    ) -> anyhow::Result<String> {
        let mut cmd = if cfg!(windows) {
            let ext = script
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if ext == "ps1" {
                let mut c = tokio::process::Command::new("powershell");
                c.arg("-NoProfile")
                    .arg("-ExecutionPolicy")
                    .arg("Bypass")
                    .arg("-File")
                    .arg(script);
                c
            } else if ext == "bat" || ext == "cmd" {
                let mut c = tokio::process::Command::new("cmd");
                c.arg("/C").arg(script);
                c
            } else {
                tokio::process::Command::new(script)
            }
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg(script);
            c
        };

        cmd.current_dir(&self.workspace_dir);

        // Populate standardized hook environment variables
        cmd.env("AGENDEX_HOOK_POINT", event.point().as_str());
        cmd.env("AGENDA_HOOK_POINT", event.point().as_str());
        cmd.env("AGENDEX_WORKSPACE", &self.workspace_dir);
        cmd.env("AGENDA_WORKSPACE", &self.workspace_dir);

        match event {
            HookEvent::PreTurn { user_input } => {
                cmd.env("AGENDEX_USER_INPUT", user_input);
                cmd.env("AGENDA_USER_INPUT", user_input);
            }
            HookEvent::PostTool {
                tool_name,
                tool_result,
            } => {
                cmd.env("AGENDEX_TOOL_NAME", tool_name);
                cmd.env("AGENDA_TOOL_NAME", tool_name);
                cmd.env("AGENDEX_TOOL_RESULT", tool_result);
                cmd.env("AGENDA_TOOL_RESULT", tool_result);
            }
            HookEvent::WorkspaceChange { old_path, new_path } => {
                cmd.env("AGENDEX_OLD_WORKSPACE", old_path);
                cmd.env("AGENDA_OLD_WORKSPACE", old_path);
                cmd.env("AGENDEX_NEW_WORKSPACE", new_path);
                cmd.env("AGENDA_NEW_WORKSPACE", new_path);
            }
            HookEvent::Stop { reason } => {
                cmd.env("AGENDEX_STOP_REASON", reason);
                cmd.env("AGENDA_STOP_REASON", reason);
            }
        }

        // Execute bounded by timeout
        let child = cmd.output();
        let output = tokio::time::timeout(self.timeout, child).await??;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let err_msg = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else {
                format!("Process exited with status code {:?}", output.status.code())
            };
            anyhow::bail!(err_msg);
        }

        Ok(stdout.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_programmatic_pre_turn_and_post_tool_dispatch() {
        let temp_dir =
            std::env::temp_dir().join(format!("agenda_hooks_prog_{}", std::process::id()));
        let dispatcher = HookDispatcher::new(&temp_dir).await.unwrap();

        // Register PreTurn hook
        dispatcher
            .register_fn(HookPoint::PreTurn, "input_validator", |event| async move {
                if let HookEvent::PreTurn { user_input } = event {
                    Ok(format!("Validated: {}", user_input))
                } else {
                    anyhow::bail!("Unexpected event");
                }
            })
            .await;

        // Register PostTool hook
        dispatcher
            .register_fn(HookPoint::PostTool, "audit_logger", |event| async move {
                if let HookEvent::PostTool { tool_name, .. } = event {
                    Ok(format!("Logged tool: {}", tool_name))
                } else {
                    anyhow::bail!("Unexpected event");
                }
            })
            .await;

        // Trigger PreTurn
        let summary = dispatcher.trigger_pre_turn("Hello world").await.unwrap();
        assert!(summary.all_succeeded);
        assert_eq!(summary.executions.len(), 1);
        assert_eq!(summary.executions[0].hook_name, "input_validator");
        assert_eq!(summary.executions[0].output, "Validated: Hello world");

        // Trigger PostTool
        let post_summary = dispatcher
            .trigger_post_tool("write_file", "success")
            .await
            .unwrap();
        assert!(post_summary.all_succeeded);
        assert_eq!(post_summary.executions.len(), 1);
        assert_eq!(post_summary.executions[0].hook_name, "audit_logger");
        assert_eq!(post_summary.executions[0].output, "Logged tool: write_file");

        let _ = tokio::fs::remove_dir_all(temp_dir).await;
    }

    #[tokio::test]
    async fn test_hook_failure_isolation() {
        let temp_dir =
            std::env::temp_dir().join(format!("agenda_hooks_fail_{}", std::process::id()));
        let dispatcher = HookDispatcher::new(&temp_dir).await.unwrap();

        dispatcher
            .register_fn(HookPoint::PreTurn, "failing_hook", |_| async {
                anyhow::bail!("Forced verification error");
            })
            .await;

        let summary = dispatcher.trigger_pre_turn("Test input").await.unwrap();
        assert!(!summary.all_succeeded);
        assert_eq!(summary.executions.len(), 1);
        assert!(!summary.executions[0].success);
        assert!(summary.executions[0]
            .output
            .contains("Forced verification error"));

        let _ = tokio::fs::remove_dir_all(temp_dir).await;
    }

    #[tokio::test]
    async fn test_script_hook_discovery_and_execution() {
        let temp_dir =
            std::env::temp_dir().join(format!("agenda_hooks_scr_{}", std::process::id()));
        let hooks_dir = temp_dir.join(".agenda").join("hooks");
        tokio::fs::create_dir_all(&hooks_dir).await.unwrap();

        if cfg!(windows) {
            let hook_file = hooks_dir.join("stop.bat");
            tokio::fs::write(&hook_file, "@echo off\necho Stop hook finished\n")
                .await
                .unwrap();
        } else {
            let hook_file = hooks_dir.join("stop.sh");
            tokio::fs::write(&hook_file, "#!/bin/sh\necho Stop hook finished\n")
                .await
                .unwrap();
        }

        let dispatcher = HookDispatcher::new(&temp_dir).await.unwrap();
        let summary = dispatcher.trigger_stop("Task completed").await.unwrap();

        assert!(summary.all_succeeded);
        assert_eq!(summary.executions.len(), 1);
        assert!(summary.executions[0].output.contains("Stop hook finished"));

        let _ = tokio::fs::remove_dir_all(temp_dir).await;
    }
}
