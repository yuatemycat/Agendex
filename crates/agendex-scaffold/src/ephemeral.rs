use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

/// Supported language runtimes for ephemeral tool scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolRuntime {
    Python,
    PowerShell,
    Batch,
    Bash,
    Node,
    Rust,
    Binary,
}

impl ToolRuntime {
    /// Returns the standard lowercase string representation of the runtime.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::PowerShell => "powershell",
            Self::Batch => "batch",
            Self::Bash => "bash",
            Self::Node => "node",
            Self::Rust => "rust",
            Self::Binary => "binary",
        }
    }

    /// Default file extension for script files created under this runtime.
    pub fn default_extension(&self) -> &'static str {
        match self {
            Self::Python => "py",
            Self::PowerShell => "ps1",
            Self::Batch => "bat",
            Self::Bash => "sh",
            Self::Node => "js",
            Self::Rust => "rs",
            Self::Binary => {
                if cfg!(windows) {
                    "exe"
                } else {
                    ""
                }
            }
        }
    }

    /// Loosely parses a runtime identifier from user input or file extensions.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "python" | "py" | "python3" => Some(Self::Python),
            "powershell" | "ps1" | "pwsh" => Some(Self::PowerShell),
            "batch" | "bat" | "cmd" => Some(Self::Batch),
            "bash" | "sh" | "shell" => Some(Self::Bash),
            "node" | "javascript" | "js" => Some(Self::Node),
            "rust" | "rs" => Some(Self::Rust),
            "binary" | "bin" | "exe" => Some(Self::Binary),
            _ => None,
        }
    }
}

impl fmt::Display for ToolRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for ToolRuntime {
    type Err = EphemeralError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_loose(s).ok_or_else(|| {
            EphemeralError::Other(format!("Unrecognized or unsupported tool runtime: '{}'", s))
        })
    }
}

/// Errors occurring during ephemeral tool synthesis, compilation, or execution.
#[derive(Debug, Error)]
pub enum EphemeralError {
    #[error("Tool '{0}' already exists in registry")]
    ToolAlreadyExists(String),

    #[error("Tool '{0}' not found in registry")]
    ToolNotFound(String),

    #[error("Invalid tool name '{0}': name must only contain alphanumeric characters, underscores, or hyphens")]
    InvalidName(String),

    #[error("Failed to write tool script to '{path}': {source}")]
    ScriptWriteError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Compilation failed for tool '{name}': {output}")]
    CompilationFailed { name: String, output: String },

    #[error(
        "Verification execution failed for tool '{name}': exit code {exit_code:?}, stderr: {stderr}"
    )]
    VerificationFailed {
        name: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("Tool '{name}' execution timed out after {timeout_ms} ms")]
    ExecutionTimeout { name: String, timeout_ms: u64 },

    #[error("Unsupported runtime or missing interpreter for {runtime:?}: {reason}")]
    RuntimeUnavailable {
        runtime: ToolRuntime,
        reason: String,
    },

    #[error("Tool execution process spawn failed: {0}")]
    ProcessSpawnError(#[from] std::io::Error),

    #[error("Ephemeral tool error: {0}")]
    Other(String),
}

/// Represents an ephemeral, on-demand synthesized tool in the workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EphemeralTool {
    /// Unique identifier / tool name.
    pub name: String,
    /// Human-readable purpose of this ephemeral tool.
    pub description: String,
    /// JSON Schema defining valid input parameters.
    pub parameters: serde_json::Value,
    /// Language runtime used to execute this tool.
    pub runtime: ToolRuntime,
    /// Source code of the synthesized script.
    pub code: String,
    /// Absolute path to the written script file.
    pub script_path: PathBuf,
    /// Optional compiled binary path (for Rust or compiled binaries).
    pub binary_path: Option<PathBuf>,
    /// Creation timestamp (seconds since UNIX epoch).
    pub created_at: u64,
}

impl EphemeralTool {
    /// Formats the tool metadata as an OpenAI function definition JSON structure.
    pub fn to_openai_spec(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            }
        })
    }

    /// Formats the tool metadata as a Gemini function declaration structure.
    pub fn to_gemini_spec(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        })
    }
}

/// Structured execution result from running an ephemeral tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    /// Name of the executed tool.
    pub tool_name: String,
    /// True if the process completed successfully with exit code 0 and did not time out.
    pub success: bool,
    /// Process exit code, or None if terminated by signal or timeout.
    pub exit_code: Option<i32>,
    /// Standard output text captured from the process.
    pub stdout: String,
    /// Standard error text captured from the process.
    pub stderr: String,
    /// Total wall-clock execution duration in milliseconds.
    pub execution_time_ms: u64,
    /// True if execution was terminated due to exceeding timeout bounds.
    pub timed_out: bool,
}

impl ToolExecutionResult {
    /// Returns stdout if non-empty, otherwise returns stderr.
    pub fn output_text(&self) -> &str {
        if !self.stdout.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        }
    }

    /// Returns a combined representation of stdout and stderr.
    pub fn combined_output(&self) -> String {
        if self.stderr.trim().is_empty() {
            self.stdout.clone()
        } else if self.stdout.trim().is_empty() {
            self.stderr.clone()
        } else {
            format!("STDOUT:\n{}\nSTDERR:\n{}", self.stdout, self.stderr)
        }
    }
}

/// Specification used to synthesize and optionally verify an ephemeral tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSynthesisSpec {
    pub name: String,
    pub description: String,
    pub runtime: ToolRuntime,
    pub code: String,
    pub parameters: Option<serde_json::Value>,
    pub fixture_args: Option<Vec<String>>,
    pub verify: bool,
    pub timeout: Option<Duration>,
    pub require_json_stdout: bool,
}

impl ToolSynthesisSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        runtime: ToolRuntime,
        code: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            runtime,
            code: code.into(),
            parameters: None,
            fixture_args: None,
            verify: true,
            timeout: None,
            require_json_stdout: false,
        }
    }

    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = Some(parameters);
        self
    }

    pub fn with_fixture_args(mut self, args: Vec<String>) -> Self {
        self.fixture_args = Some(args);
        self
    }

    pub fn with_verify(mut self, verify: bool) -> Self {
        self.verify = verify;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn with_require_json_stdout(mut self, require: bool) -> Self {
        self.require_json_stdout = require;
        self
    }
}

/// Registry managing ephemeral tool synthesis, lifecycle, child process execution, and scratch storage.
#[derive(Debug, Clone)]
pub struct EphemeralToolRegistry {
    workspace_dir: PathBuf,
    scratch_dir: PathBuf,
    default_timeout: Duration,
    tools: Arc<RwLock<HashMap<String, EphemeralTool>>>,
}

impl EphemeralToolRegistry {
    /// Creates a new EphemeralToolRegistry for the given workspace.
    ///
    /// By default, scripts are stored in `<workspace>/.agendex/scratch/` (or `<workspace>/.agenda/scratch/`).
    pub async fn new(workspace_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let ws = workspace_dir.as_ref().to_path_buf();
        let agendex_dir = if ws.join(".agenda").exists() && !ws.join(".agendex").exists() {
            ws.join(".agenda")
        } else {
            ws.join(".agendex")
        };
        let scratch_dir = agendex_dir.join("scratch");
        tokio::fs::create_dir_all(&scratch_dir).await?;

        Ok(Self {
            workspace_dir: ws,
            scratch_dir,
            default_timeout: Duration::from_secs(30),
            tools: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Creates an EphemeralToolRegistry synchronously without immediately creating directories on disk.
    pub fn new_sync(workspace_dir: impl AsRef<Path>) -> Self {
        let ws = workspace_dir.as_ref().to_path_buf();
        let agendex_dir = if ws.join(".agenda").exists() && !ws.join(".agendex").exists() {
            ws.join(".agenda")
        } else {
            ws.join(".agendex")
        };
        let scratch_dir = agendex_dir.join("scratch");

        Self {
            workspace_dir: ws,
            scratch_dir,
            default_timeout: Duration::from_secs(30),
            tools: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Customizes the scratch directory where temporary tools are placed.
    pub fn with_scratch_dir(mut self, scratch_dir: impl AsRef<Path>) -> Self {
        self.scratch_dir = scratch_dir.as_ref().to_path_buf();
        self
    }

    /// Customizes the default execution timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Returns the workspace root path.
    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Returns the active scratch directory.
    pub fn scratch_dir(&self) -> &Path {
        &self.scratch_dir
    }

    /// Returns the default timeout duration.
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }

    /// Synthesizes an ephemeral tool and performs verification by default.
    pub async fn synthesize_tool(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        runtime: ToolRuntime,
        code: impl Into<String>,
        parameters: Option<serde_json::Value>,
    ) -> anyhow::Result<EphemeralTool> {
        let spec = ToolSynthesisSpec {
            name: name.into(),
            description: description.into(),
            runtime,
            code: code.into(),
            parameters,
            fixture_args: None,
            verify: true,
            timeout: None,
            require_json_stdout: false,
        };
        self.synthesize_with_spec(spec).await
    }

    /// Synthesizes an ephemeral tool according to the full configuration specification.
    pub async fn synthesize_with_spec(
        &self,
        spec: ToolSynthesisSpec,
    ) -> anyhow::Result<EphemeralTool> {
        let name = spec.name.trim().to_string();

        // 1. Validate tool name
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(EphemeralError::InvalidName(name).into());
        }

        // 2. Guard against duplicate registration
        {
            let map = self.tools.read().await;
            if map.contains_key(&name) {
                return Err(EphemeralError::ToolAlreadyExists(name).into());
            }
        }

        // 3. Ensure scratch directory exists
        tokio::fs::create_dir_all(&self.scratch_dir).await?;

        // 4. Derive script filename and path
        let ext = spec.runtime.default_extension();
        let script_filename = if ext.is_empty() {
            name.clone()
        } else {
            format!("{}.{}", name, ext)
        };
        let script_path = self.scratch_dir.join(script_filename);

        // 5. Write script safely to disk
        tokio::fs::write(&script_path, spec.code.as_bytes())
            .await
            .map_err(|e| EphemeralError::ScriptWriteError {
                path: script_path.clone(),
                source: e,
            })?;

        // 6. Set executable permissions if on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = tokio::fs::metadata(&script_path).await {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                let _ = tokio::fs::set_permissions(&script_path, perms).await;
            }
        }

        // 7. Handle compilation for compiled runtimes (Rust)
        let binary_path = if spec.runtime == ToolRuntime::Rust {
            let bin_name = if cfg!(windows) {
                format!("{}.exe", name)
            } else {
                name.clone()
            };
            let out_bin = self.scratch_dir.join(bin_name);

            if let Err(e) =
                compile_rust_snippet(&script_path, &out_bin, Duration::from_secs(30)).await
            {
                let _ = tokio::fs::remove_file(&script_path).await;
                let _ = tokio::fs::remove_file(&out_bin).await;
                return Err(e);
            }
            Some(out_bin)
        } else {
            None
        };

        // 8. Construct tool instance
        let parameters = spec.parameters.unwrap_or_else(|| {
            serde_json::json!({
                "type": "object",
                "properties": {}
            })
        });

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let tool = EphemeralTool {
            name: name.clone(),
            description: spec.description,
            parameters,
            runtime: spec.runtime,
            code: spec.code,
            script_path: script_path.clone(),
            binary_path: binary_path.clone(),
            created_at,
        };

        // 9. Run verification dry-run if requested
        if spec.verify {
            let fixture_args = spec.fixture_args.unwrap_or_default();
            let verification_timeout = spec.timeout.unwrap_or(Duration::from_secs(5));

            let verify_res = self
                .execute_tool_raw(&tool, &fixture_args, &HashMap::new(), verification_timeout)
                .await?;

            if !verify_res.success || verify_res.exit_code != Some(0) {
                // Verification failed: cleanup files and refuse registration
                let _ = tokio::fs::remove_file(&tool.script_path).await;
                if let Some(ref bin) = tool.binary_path {
                    let _ = tokio::fs::remove_file(bin).await;
                }
                return Err(EphemeralError::VerificationFailed {
                    name,
                    exit_code: verify_res.exit_code,
                    stderr: verify_res.stderr,
                }
                .into());
            }

            if spec.require_json_stdout
                && serde_json::from_str::<serde_json::Value>(&verify_res.stdout).is_err()
            {
                let _ = tokio::fs::remove_file(&tool.script_path).await;
                if let Some(ref bin) = tool.binary_path {
                    let _ = tokio::fs::remove_file(bin).await;
                }
                return Err(EphemeralError::VerificationFailed {
                    name,
                    exit_code: verify_res.exit_code,
                    stderr: "Verification failed: stdout is not valid JSON".to_string(),
                }
                .into());
            }
        }

        // 10. Register tool in the in-memory map
        {
            let mut map = self.tools.write().await;
            map.insert(name, tool.clone());
        }

        Ok(tool)
    }

    /// Executes a registered ephemeral tool by name with arguments and optional timeout.
    pub async fn execute_tool(
        &self,
        name: &str,
        args: &[String],
        timeout: Option<Duration>,
    ) -> anyhow::Result<ToolExecutionResult> {
        self.execute_tool_with_env(name, args, &HashMap::new(), timeout)
            .await
    }

    /// Convenience wrapper accepting `&[&str]` arguments.
    pub async fn execute_tool_str(
        &self,
        name: &str,
        args: &[&str],
        timeout: Option<Duration>,
    ) -> anyhow::Result<ToolExecutionResult> {
        let owned_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.execute_tool(name, &owned_args, timeout).await
    }

    /// Executes a registered ephemeral tool with custom environment variables and timeout bounds.
    pub async fn execute_tool_with_env(
        &self,
        name: &str,
        args: &[String],
        env: &HashMap<String, String>,
        timeout: Option<Duration>,
    ) -> anyhow::Result<ToolExecutionResult> {
        let tool = {
            let map = self.tools.read().await;
            map.get(name)
                .cloned()
                .ok_or_else(|| EphemeralError::ToolNotFound(name.to_string()))?
        };

        let effective_timeout = timeout.unwrap_or(self.default_timeout);
        self.execute_tool_raw(&tool, args, env, effective_timeout)
            .await
    }

    /// Internal raw executor for an EphemeralTool instance.
    async fn execute_tool_raw(
        &self,
        tool: &EphemeralTool,
        args: &[String],
        env: &HashMap<String, String>,
        timeout: Duration,
    ) -> anyhow::Result<ToolExecutionResult> {
        let mut cmd = match tool.runtime {
            ToolRuntime::Python => {
                let py = resolve_python_command();
                let mut c = tokio::process::Command::new(py);
                c.arg(&tool.script_path);
                c.args(args);
                c.env("PYTHONUNBUFFERED", "1");
                c
            }
            ToolRuntime::PowerShell => {
                let ps = resolve_powershell_command();
                let mut c = tokio::process::Command::new(ps);
                if cfg!(windows) {
                    c.arg("-NoProfile")
                        .arg("-ExecutionPolicy")
                        .arg("Bypass")
                        .arg("-File")
                        .arg(&tool.script_path);
                } else {
                    c.arg("-NoProfile").arg("-File").arg(&tool.script_path);
                }
                c.args(args);
                c
            }
            ToolRuntime::Batch => {
                if !cfg!(windows) {
                    return Err(EphemeralError::RuntimeUnavailable {
                        runtime: ToolRuntime::Batch,
                        reason: "Batch scripts are only supported on Windows".to_string(),
                    }
                    .into());
                }
                let mut c = tokio::process::Command::new("cmd");
                c.arg("/C").arg(&tool.script_path).args(args);
                c
            }
            ToolRuntime::Bash => {
                let bash = resolve_bash_command();
                let mut c = tokio::process::Command::new(bash);
                c.arg(&tool.script_path).args(args);
                c
            }
            ToolRuntime::Node => {
                let mut c = tokio::process::Command::new("node");
                c.arg(&tool.script_path).args(args);
                c
            }
            ToolRuntime::Rust => {
                let exec_path = tool.binary_path.as_ref().unwrap_or(&tool.script_path);
                let mut c = tokio::process::Command::new(exec_path);
                c.args(args);
                c
            }
            ToolRuntime::Binary => {
                let exec_path = tool.binary_path.as_ref().unwrap_or(&tool.script_path);
                let mut c = tokio::process::Command::new(exec_path);
                c.args(args);
                c
            }
        };

        cmd.current_dir(&self.workspace_dir);
        cmd.kill_on_drop(true);

        // Standardized sandbox environment variables
        cmd.env("AGENDEX_WORKSPACE", &self.workspace_dir);
        cmd.env("AGENDEX_SCRATCH", &self.scratch_dir);
        cmd.env("AGENDEX_TOOL_NAME", &tool.name);
        cmd.env("AGENDEX_TOOL_RUNTIME", tool.runtime.as_str());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let start = Instant::now();
        let child = cmd.output();

        match tokio::time::timeout(timeout, child).await {
            Ok(Ok(output)) => {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let success = output.status.success();
                let exit_code = output.status.code();

                Ok(ToolExecutionResult {
                    tool_name: tool.name.clone(),
                    success,
                    exit_code,
                    stdout,
                    stderr,
                    execution_time_ms: elapsed_ms,
                    timed_out: false,
                })
            }
            Ok(Err(e)) => Err(EphemeralError::ProcessSpawnError(e).into()),
            Err(_) => {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                Ok(ToolExecutionResult {
                    tool_name: tool.name.clone(),
                    success: false,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: format!("Tool execution timed out after {} ms", elapsed_ms),
                    execution_time_ms: elapsed_ms,
                    timed_out: true,
                })
            }
        }
    }

    /// Returns a list of all currently registered ephemeral tools sorted by name.
    pub async fn list_tools(&self) -> Vec<EphemeralTool> {
        let map = self.tools.read().await;
        let mut list: Vec<EphemeralTool> = map.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Looks up a registered ephemeral tool by name.
    pub async fn get_tool(&self, name: &str) -> Option<EphemeralTool> {
        let map = self.tools.read().await;
        map.get(name).cloned()
    }

    /// Returns true if a tool with the given name is registered.
    pub async fn has_tool(&self, name: &str) -> bool {
        let map = self.tools.read().await;
        map.contains_key(name)
    }

    /// Removes an ephemeral tool by name and securely deletes its script and binary files from disk.
    pub async fn remove_tool(&self, name: &str) -> anyhow::Result<bool> {
        let tool_opt = {
            let mut map = self.tools.write().await;
            map.remove(name)
        };

        if let Some(tool) = tool_opt {
            if tool.script_path.exists() {
                let _ = tokio::fs::remove_file(&tool.script_path).await;
            }
            if let Some(ref bin) = tool.binary_path {
                if bin.exists() {
                    let _ = tokio::fs::remove_file(bin).await;
                }
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Removes all ephemeral tools and purges their files from disk.
    pub async fn clear(&self) -> anyhow::Result<usize> {
        let tools: Vec<EphemeralTool> = {
            let mut map = self.tools.write().await;
            map.drain().map(|(_, v)| v).collect()
        };

        let count = tools.len();
        for tool in tools {
            if tool.script_path.exists() {
                let _ = tokio::fs::remove_file(&tool.script_path).await;
            }
            if let Some(ref bin) = tool.binary_path {
                if bin.exists() {
                    let _ = tokio::fs::remove_file(bin).await;
                }
            }
        }

        Ok(count)
    }

    /// Returns the number of currently registered ephemeral tools.
    pub async fn len(&self) -> usize {
        let map = self.tools.read().await;
        map.len()
    }

    /// Returns true if no ephemeral tools are registered.
    pub async fn is_empty(&self) -> bool {
        let map = self.tools.read().await;
        map.is_empty()
    }
}

/// Helper function to compile a Rust script snippet with `rustc`.
async fn compile_rust_snippet(
    script_path: &Path,
    binary_path: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new("rustc");
    cmd.arg("--edition")
        .arg("2021")
        .arg(script_path)
        .arg("-o")
        .arg(binary_path);
    cmd.kill_on_drop(true);

    let child = cmd.output();
    let output = match tokio::time::timeout(timeout, child).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Err(EphemeralError::CompilationFailed {
                name: script_path.display().to_string(),
                output: format!("Failed to spawn rustc compiler: {}", e),
            }
            .into());
        }
        Err(_) => {
            return Err(EphemeralError::CompilationFailed {
                name: script_path.display().to_string(),
                output: "rustc compilation timed out".to_string(),
            }
            .into());
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(EphemeralError::CompilationFailed {
            name: script_path.display().to_string(),
            output: stderr.trim().to_string(),
        }
        .into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = tokio::fs::metadata(binary_path).await {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = tokio::fs::set_permissions(binary_path, perms).await;
        }
    }

    Ok(())
}

fn resolve_python_command() -> &'static str {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "python3"
    } else if std::process::Command::new("python")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "python"
    } else if std::process::Command::new("py")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "py"
    } else if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn resolve_powershell_command() -> &'static str {
    if cfg!(windows) {
        "powershell"
    } else {
        "pwsh"
    }
}

fn resolve_bash_command() -> &'static str {
    if cfg!(windows) {
        if Path::new(r"C:\Program Files\Git\bin\bash.exe").exists() {
            r"C:\Program Files\Git\bin\bash.exe"
        } else {
            "bash"
        }
    } else {
        "sh"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = env::temp_dir().join(format!(
            "agendex_test_{}_{}_{}",
            prefix,
            std::process::id(),
            ts
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn test_runtime_parsing_and_metadata() {
        assert_eq!(
            ToolRuntime::from_str_loose("python"),
            Some(ToolRuntime::Python)
        );
        assert_eq!(ToolRuntime::from_str_loose("py"), Some(ToolRuntime::Python));
        assert_eq!(
            ToolRuntime::from_str_loose("powershell"),
            Some(ToolRuntime::PowerShell)
        );
        assert_eq!(
            ToolRuntime::from_str_loose("ps1"),
            Some(ToolRuntime::PowerShell)
        );
        assert_eq!(
            ToolRuntime::from_str_loose("batch"),
            Some(ToolRuntime::Batch)
        );
        assert_eq!(ToolRuntime::from_str_loose("cmd"), Some(ToolRuntime::Batch));
        assert_eq!(ToolRuntime::from_str_loose("bash"), Some(ToolRuntime::Bash));
        assert_eq!(ToolRuntime::from_str_loose("sh"), Some(ToolRuntime::Bash));
        assert_eq!(ToolRuntime::from_str_loose("node"), Some(ToolRuntime::Node));
        assert_eq!(ToolRuntime::from_str_loose("js"), Some(ToolRuntime::Node));
        assert_eq!(ToolRuntime::from_str_loose("rust"), Some(ToolRuntime::Rust));
        assert_eq!(ToolRuntime::from_str_loose("rs"), Some(ToolRuntime::Rust));
        assert_eq!(
            ToolRuntime::from_str_loose("binary"),
            Some(ToolRuntime::Binary)
        );
        assert_eq!(ToolRuntime::from_str_loose("invalid_xyz"), None);

        assert_eq!(ToolRuntime::Python.default_extension(), "py");
        assert_eq!(ToolRuntime::PowerShell.default_extension(), "ps1");
        assert_eq!(ToolRuntime::Batch.default_extension(), "bat");
        assert_eq!(ToolRuntime::Bash.default_extension(), "sh");
        assert_eq!(ToolRuntime::Node.default_extension(), "js");
        assert_eq!(ToolRuntime::Rust.default_extension(), "rs");
    }

    #[tokio::test]
    async fn test_tool_creation_and_registration() {
        let temp_dir = temp_test_dir("create_reg");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let code = "print('{\"status\": \"initialized\"}')";
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "target": { "type": "string" }
            }
        });

        let tool = registry
            .synthesize_tool(
                "init_probe",
                "Diagnostic probe tool",
                ToolRuntime::Python,
                code,
                Some(params.clone()),
            )
            .await
            .expect("Failed to synthesize tool");

        assert_eq!(tool.name, "init_probe");
        assert_eq!(tool.runtime, ToolRuntime::Python);
        assert_eq!(tool.parameters, params);
        assert!(tool.script_path.exists());

        assert!(registry.has_tool("init_probe").await);
        assert_eq!(registry.len().await, 1);

        let retrieved = registry
            .get_tool("init_probe")
            .await
            .expect("Tool not found");
        assert_eq!(retrieved.name, "init_probe");
        assert_eq!(retrieved.description, "Diagnostic probe tool");

        let tools = registry.list_tools().await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "init_probe");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_successful_script_execution_capturing_stdout_and_args() {
        let temp_dir = temp_test_dir("exec_stdout");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let code = r#"
import sys
args = sys.argv[1:]
print(f"args_received: {','.join(args)}")
"#;

        let _ = registry
            .synthesize_tool(
                "arg_echo",
                "Echoes arguments passed to it",
                ToolRuntime::Python,
                code,
                None,
            )
            .await
            .expect("Synthesis failed");

        let res = registry
            .execute_tool_str("arg_echo", &["alpha", "beta", "gamma"], None)
            .await
            .expect("Execution failed");

        assert!(res.success);
        assert_eq!(res.exit_code, Some(0));
        assert!(!res.timed_out);
        assert!(
            res.stdout.contains("args_received: alpha,beta,gamma"),
            "Stdout did not contain expected text: {}",
            res.stdout
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_timeout_enforcement_for_hanging_scripts() {
        let temp_dir = temp_test_dir("timeout");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let code = r#"
import time
time.sleep(10)
print("finished sleep")
"#;

        let spec = ToolSynthesisSpec::new(
            "hang_tool",
            "Simulates long running or hung script",
            ToolRuntime::Python,
            code,
        )
        .with_verify(false);

        let _ = registry
            .synthesize_with_spec(spec)
            .await
            .expect("Synthesis should succeed with verify:false");

        let start = Instant::now();
        let res = registry
            .execute_tool("hang_tool", &[], Some(Duration::from_millis(300)))
            .await
            .expect("execute_tool should return structured result on timeout");

        let elapsed = start.elapsed();
        assert!(res.timed_out, "Expected result to be marked as timed_out");
        assert!(!res.success, "Timed out command should not succeed");
        assert_eq!(res.exit_code, None);
        assert!(
            res.stderr.contains("timed out"),
            "Expected timeout message in stderr: {}",
            res.stderr
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "Execution should be terminated close to timeout bound, took {:?}",
            elapsed
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_clean_deletion_and_error_handling() {
        let temp_dir = temp_test_dir("cleanup");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let code = "print('temporary diagnostic')";
        let tool = registry
            .synthesize_tool(
                "cleanup_tool",
                "Tool to be cleaned up",
                ToolRuntime::Python,
                code,
                None,
            )
            .await
            .expect("Synthesis failed");

        assert!(tool.script_path.exists());
        assert!(registry.has_tool("cleanup_tool").await);

        let removed = registry
            .remove_tool("cleanup_tool")
            .await
            .expect("Remove tool failed");
        assert!(removed);

        assert!(!registry.has_tool("cleanup_tool").await);
        assert!(registry.get_tool("cleanup_tool").await.is_none());
        assert!(
            !tool.script_path.exists(),
            "Script file must be removed from scratch directory"
        );

        // Second remove should return false
        let removed_again = registry
            .remove_tool("cleanup_tool")
            .await
            .expect("Remove tool failed");
        assert!(!removed_again);

        // Executing non-existent tool should return Err(ToolNotFound)
        let exec_res = registry.execute_tool_str("cleanup_tool", &[], None).await;
        assert!(exec_res.is_err());
        let err_msg = exec_res.err().unwrap().to_string();
        assert!(err_msg.contains("not found in registry"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_invalid_name_rejected() {
        let temp_dir = temp_test_dir("invalid_name");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let code = "print('noop')";
        let res = registry
            .synthesize_tool(
                "../path_traversal",
                "Illegal name",
                ToolRuntime::Python,
                code,
                None,
            )
            .await;

        assert!(res.is_err());
        let err = res.err().unwrap().to_string();
        assert!(err.contains("Invalid tool name"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_verification_failure_cleans_up_and_rejects_registration() {
        let temp_dir = temp_test_dir("verify_fail");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let code = r#"
import sys
sys.stderr.write("Script raised critical fault\n")
sys.exit(1)
"#;

        let res = registry
            .synthesize_tool(
                "faulty_tool",
                "Failing tool",
                ToolRuntime::Python,
                code,
                None,
            )
            .await;

        assert!(
            res.is_err(),
            "Tool synthesis with failing exit code must fail"
        );
        let err = res.err().unwrap().to_string();
        assert!(err.contains("Verification execution failed"));
        assert!(err.contains("Script raised critical fault"));

        // Script must not be registered
        assert!(!registry.has_tool("faulty_tool").await);
        assert_eq!(registry.len().await, 0);

        // Script file must not remain on disk
        let expected_path = registry.scratch_dir().join("faulty_tool.py");
        assert!(
            !expected_path.exists(),
            "Faulty tool script must be cleaned up on verification failure"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_clear_purges_all_tools_and_files() {
        let temp_dir = temp_test_dir("clear_all");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let tool1 = registry
            .synthesize_tool("tool_a", "A", ToolRuntime::Python, "print('a')", None)
            .await
            .unwrap();
        let tool2 = registry
            .synthesize_tool("tool_b", "B", ToolRuntime::Python, "print('b')", None)
            .await
            .unwrap();

        assert_eq!(registry.len().await, 2);
        assert!(tool1.script_path.exists());
        assert!(tool2.script_path.exists());

        let cleared_count = registry.clear().await.unwrap();
        assert_eq!(cleared_count, 2);
        assert_eq!(registry.len().await, 0);
        assert!(!tool1.script_path.exists());
        assert!(!tool2.script_path.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_tool_json_spec_generation() {
        let temp_dir = temp_test_dir("specs");
        let registry = EphemeralToolRegistry::new(&temp_dir)
            .await
            .expect("Failed to initialize registry");

        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "metric": { "type": "string" }
            },
            "required": ["metric"]
        });

        let tool = registry
            .synthesize_tool(
                "query_metric",
                "Queries system metrics",
                ToolRuntime::Python,
                "print('metric')",
                Some(params.clone()),
            )
            .await
            .unwrap();

        let openai_spec = tool.to_openai_spec();
        assert_eq!(openai_spec["type"], "function");
        assert_eq!(openai_spec["function"]["name"], "query_metric");
        assert_eq!(
            openai_spec["function"]["description"],
            "Queries system metrics"
        );
        assert_eq!(openai_spec["function"]["parameters"], params);

        let gemini_spec = tool.to_gemini_spec();
        assert_eq!(gemini_spec["name"], "query_metric");
        assert_eq!(gemini_spec["description"], "Queries system metrics");
        assert_eq!(gemini_spec["parameters"], params);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
