use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

use crate::agent::{AgendexAgent, AgentConfig, ApprovalHandler};
use crate::mailbox::{MailboxEvent, MailboxManager};
use crate::worktree::{WorkspaceMode, WorktreeManager};
use agendex_trajectory::compactor::ModelTier;

/// Specification of a dynamic subagent persona including permissions and model tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentPersona {
    pub name: String,
    pub role: String,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub model_tier: ModelTier,
}

impl SubagentPersona {
    pub fn new(
        name: impl Into<String>,
        role: impl Into<String>,
        system_prompt: impl Into<String>,
        allowed_tools: Vec<String>,
        model_tier: ModelTier,
    ) -> Self {
        Self {
            name: name.into(),
            role: role.into(),
            system_prompt: system_prompt.into(),
            allowed_tools,
            model_tier,
        }
    }

    /// Read-only research and exploration persona.
    pub fn researcher() -> Self {
        Self {
            name: "researcher".to_string(),
            role: "Codebase & Documentation Researcher".to_string(),
            system_prompt:
                "You are a read-only research specialist. Inspect files and explain architectures."
                    .to_string(),
            allowed_tools: vec![
                "read_file".to_string(),
                "list_directory".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "send_message".to_string(),
            ],
            model_tier: ModelTier::Standard128k,
        }
    }

    /// Full implementation persona with write and command permissions.
    pub fn coder() -> Self {
        Self {
            name: "coder".to_string(),
            role: "Software Implementation Engineer".to_string(),
            system_prompt: "You are a software engineer specializing in surgical code implementation and bug fixes.".to_string(),
            allowed_tools: vec![
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "list_directory".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "run_command".to_string(),
                "send_message".to_string(),
            ],
            model_tier: ModelTier::HighTier256k,
        }
    }

    /// Code quality and security review persona.
    pub fn reviewer() -> Self {
        Self {
            name: "reviewer".to_string(),
            role: "Security & Code Quality Reviewer".to_string(),
            system_prompt:
                "You are an expert reviewer auditing code changes for correctness and security."
                    .to_string(),
            allowed_tools: vec![
                "read_file".to_string(),
                "list_directory".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "send_message".to_string(),
            ],
            model_tier: ModelTier::Standard128k,
        }
    }
}

/// Lifecycle execution status of a subagent instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Terminated,
}

/// Record representing a spawned subagent instance and its current execution state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentInstance {
    pub id: String,
    pub persona: SubagentPersona,
    pub status: SubagentStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub workspace_dir: PathBuf,
    pub workspace_mode: WorkspaceMode,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Shared console request message sent across threads to the dedicated terminal worker.
enum ConsoleRequest {
    Approval {
        subagent_id: String,
        command: String,
        reason: String,
        responder: tokio::sync::oneshot::Sender<bool>,
    },
    DiffApproval {
        subagent_id: String,
        path: String,
        diff: String,
        responder: tokio::sync::oneshot::Sender<bool>,
    },
    ReadLine {
        prompt: String,
        responder: tokio::sync::oneshot::Sender<Option<String>>,
    },
}

enum ConsoleEvent {
    Request(ConsoleRequest),
    Line(String),
    Eof,
}

enum ActiveApproval {
    Command {
        tx: tokio::sync::oneshot::Sender<bool>,
        command: String,
    },
    Diff {
        tx: tokio::sync::oneshot::Sender<bool>,
        path: String,
    },
}

/// Shared console coordinator serializing all interactive prompts and standard input across
/// concurrent subagents and the CLI REPL, guaranteeing exactly one reader on terminal stdin.
pub struct SharedApprovalQueue {
    sender: std::sync::mpsc::Sender<ConsoleEvent>,
    is_paused: Arc<std::sync::atomic::AtomicBool>,
    allowed_commands: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    allowed_diffs: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl SharedApprovalQueue {
    pub fn new() -> Arc<Self> {
        let (event_tx, event_rx) = std::sync::mpsc::channel::<ConsoleEvent>();
        let is_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let allowed_commands = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let allowed_diffs = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        let allowed_cmds_thread = Arc::clone(&allowed_commands);
        let allowed_diffs_thread = Arc::clone(&allowed_diffs);

        // 1. Spawn dedicated Stdin Reader Thread that respects is_paused
        let stdin_tx = event_tx.clone();
        let is_paused_reader = Arc::clone(&is_paused);
        std::thread::Builder::new()
            .name("agendex-stdin-reader".to_string())
            .spawn(move || {
                use std::io::{IsTerminal, Write};
                if std::io::stdin().is_terminal() {
                    let mut line_buf = String::new();
                    loop {
                        if is_paused_reader.load(std::sync::atomic::Ordering::SeqCst) {
                            std::thread::sleep(std::time::Duration::from_millis(40));
                            continue;
                        }

                        if crossterm::event::poll(std::time::Duration::from_millis(40))
                            .unwrap_or(false)
                        {
                            if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read()
                            {
                                if key.kind == crossterm::event::KeyEventKind::Release {
                                    continue;
                                }

                                match key.code {
                                    crossterm::event::KeyCode::Enter => {
                                        let line = std::mem::take(&mut line_buf);
                                        println!();
                                        if stdin_tx.send(ConsoleEvent::Line(line)).is_err() {
                                            break;
                                        }
                                    }
                                    crossterm::event::KeyCode::Char(c) => {
                                        line_buf.push(c);
                                        print!("{}", c);
                                        let _ = std::io::stdout().flush();
                                    }
                                    crossterm::event::KeyCode::Backspace
                                        if line_buf.pop().is_some() =>
                                    {
                                        print!("\x08 \x08");
                                        let _ = std::io::stdout().flush();
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                } else {
                    let stdin = std::io::stdin();
                    loop {
                        let mut line = String::new();
                        match stdin.read_line(&mut line) {
                            Ok(0) => {
                                let _ = stdin_tx.send(ConsoleEvent::Eof);
                                break;
                            }
                            Ok(_) => {
                                if stdin_tx.send(ConsoleEvent::Line(line)).is_err() {
                                    break;
                                }
                            }
                            Err(_) => {
                                let _ = stdin_tx.send(ConsoleEvent::Eof);
                                break;
                            }
                        }
                    }
                }
            })
            .expect("Failed to spawn stdin reader thread");

        // 2. Spawn dedicated Console Coordinator Thread that serializes all interactive prompts
        std::thread::Builder::new()
            .name("agendex-console-coordinator".to_string())
            .spawn(move || {
                use std::io::Write;
                let mut active_approval: Option<ActiveApproval> = None;
                let mut pending_approvals: std::collections::VecDeque<ConsoleRequest> =
                    std::collections::VecDeque::new();
                let mut active_readline: Option<(String, tokio::sync::oneshot::Sender<Option<String>>)> =
                    None;

                while let Ok(event) = event_rx.recv() {
                    match event {
                        ConsoleEvent::Request(req) => match req {
                            ConsoleRequest::Approval {
                                subagent_id,
                                command,
                                reason,
                                responder,
                            } => {
                                if active_approval.is_some() {
                                    pending_approvals.push_back(ConsoleRequest::Approval {
                                        subagent_id,
                                        command,
                                        reason,
                                        responder,
                                    });
                                } else {
                                    let card = crate::ui::format_safety_gate_card(
                                        &command,
                                        &reason,
                                        if subagent_id.is_empty() || subagent_id == "parent" {
                                            None
                                        } else {
                                            Some(&subagent_id)
                                        },
                                    );
                                    print!("{}", card);
                                    print!(
                                        "  {}Allow execution? [y]es / [n]o / [a]lways allow{} {}❯{} ",
                                        crate::ui::TEXT_PRIMARY,
                                        crate::ui::RESET,
                                        crate::ui::ACCENT_AMBER,
                                        crate::ui::RESET
                                    );
                                    let _ = std::io::stdout().flush();
                                    active_approval = Some(ActiveApproval::Command {
                                        tx: responder,
                                        command,
                                    });
                                }
                            }
                            ConsoleRequest::DiffApproval {
                                subagent_id,
                                path,
                                diff,
                                responder,
                            } => {
                                if active_approval.is_some() {
                                    pending_approvals.push_back(ConsoleRequest::DiffApproval {
                                        subagent_id,
                                        path,
                                        diff,
                                        responder,
                                    });
                                } else {
                                    let colored_diff = crate::ui::format_diff_colored(&path, &diff);
                                    print!("{}", colored_diff);
                                    print!(
                                        "  {}Apply this change? [y]es / [n]o / [a]lways allow{} {}❯{} ",
                                        crate::ui::TEXT_PRIMARY,
                                        crate::ui::RESET,
                                        crate::ui::ACCENT_CYAN,
                                        crate::ui::RESET
                                    );
                                    let _ = std::io::stdout().flush();
                                    active_approval = Some(ActiveApproval::Diff {
                                        tx: responder,
                                        path,
                                    });
                                }
                            }
                            ConsoleRequest::ReadLine { prompt, responder } => {
                                if active_approval.is_none() && !prompt.is_empty() {
                                    print!("{}", prompt);
                                    let _ = std::io::stdout().flush();
                                }
                                active_readline = Some((prompt, responder));
                            }
                        },
                        ConsoleEvent::Line(input) => {
                            if let Some(ticket) = active_approval.take() {
                                let trimmed = input.trim().to_lowercase();
                                let is_always = trimmed == "a" || trimmed == "always";
                                let approved = is_always || trimmed == "y" || trimmed == "yes";
                                match ticket {
                                    ActiveApproval::Command { tx, command } => {
                                        if is_always {
                                            allowed_cmds_thread
                                                .lock()
                                                .unwrap()
                                                .insert(command);
                                        }
                                        let _ = tx.send(approved);
                                    }
                                    ActiveApproval::Diff { tx, path } => {
                                        if is_always {
                                            allowed_diffs_thread
                                                .lock()
                                                .unwrap()
                                                .insert(path);
                                        }
                                        let _ = tx.send(approved);
                                    }
                                }

                                // If there are pending approvals, prompt the next one
                                if let Some(next_req) = pending_approvals.pop_front() {
                                    match next_req {
                                        ConsoleRequest::Approval {
                                            subagent_id,
                                            command,
                                            reason,
                                            responder,
                                        } => {
                                            println!();
                                            if subagent_id.is_empty() || subagent_id == "parent" {
                                                println!(
                                                    "[Safety Gate] Command is classified as risky ({}):",
                                                    reason
                                                );
                                            } else {
                                                println!(
                                                    "[Safety Gate: Subagent {}] Command is classified as risky ({}):",
                                                    subagent_id, reason
                                                );
                                            }
                                            println!("  {}", command);
                                            print!("Allow execution? [y]es / [n]o / [a]lways allow: ");
                                            let _ = std::io::stdout().flush();
                                            active_approval = Some(ActiveApproval::Command {
                                                tx: responder,
                                                command,
                                            });
                                        }
                                        ConsoleRequest::DiffApproval {
                                            subagent_id,
                                            path,
                                            diff,
                                            responder,
                                        } => {
                                            println!();
                                            if subagent_id.is_empty() || subagent_id == "parent" {
                                                println!(
                                                    "[Diff Review] Proposed changes to '{}':",
                                                    path
                                                );
                                            } else {
                                                println!(
                                                    "[Diff Review: Subagent {}] Proposed changes to '{}':",
                                                    subagent_id, path
                                                );
                                            }
                                            println!("{}", diff);
                                            print!("Apply this change? [y]es / [n]o / [a]lways allow: ");
                                            let _ = std::io::stdout().flush();
                                            active_approval = Some(ActiveApproval::Diff {
                                                tx: responder,
                                                path,
                                            });
                                        }
                                        _ => {}
                                    }
                                } else if let Some((ref prompt, _)) = active_readline {
                                    // No pending approvals left, restore the REPL prompt
                                    if !prompt.is_empty() {
                                        print!("{}", prompt);
                                        let _ = std::io::stdout().flush();
                                    }
                                }
                            } else if let Some((_, responder)) = active_readline.take() {
                                let _ = responder.send(Some(input));
                            }
                        }
                        ConsoleEvent::Eof => {
                            if let Some(ticket) = active_approval.take() {
                                match ticket {
                                    ActiveApproval::Command { tx, .. } => {
                                        let _ = tx.send(false);
                                    }
                                    ActiveApproval::Diff { tx, .. } => {
                                        let _ = tx.send(false);
                                    }
                                }
                            }
                            while let Some(req) = pending_approvals.pop_front() {
                                match req {
                                    ConsoleRequest::Approval { responder, .. } => {
                                        let _ = responder.send(false);
                                    }
                                    ConsoleRequest::DiffApproval { responder, .. } => {
                                        let _ = responder.send(false);
                                    }
                                    ConsoleRequest::ReadLine { responder, .. } => {
                                        let _ = responder.send(None);
                                    }
                                }
                            }
                            if let Some((_, responder)) = active_readline.take() {
                                let _ = responder.send(None);
                            }
                            break;
                        }
                    }
                }
            })
            .expect("Failed to spawn console coordinator thread");

        Arc::new(Self {
            sender: event_tx,
            is_paused,
            allowed_commands,
            allowed_diffs,
        })
    }

    /// Temporarily pauses the background console reader so interactive menus have exclusive terminal access.
    pub fn pause(&self) {
        self.is_paused
            .store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(60));
    }

    /// Resumes the background console reader.
    pub fn resume(&self) {
        self.is_paused
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Asynchronously wait for user approval ticket without blocking the async runtime
    pub async fn request_approval(&self, subagent_id: &str, command: &str, reason: &str) -> bool {
        if self.is_paused.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        if self.allowed_commands.lock().unwrap().contains(command) {
            return true;
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = ConsoleRequest::Approval {
            subagent_id: subagent_id.to_string(),
            command: command.to_string(),
            reason: reason.to_string(),
            responder: tx,
        };

        if self.sender.send(ConsoleEvent::Request(req)).is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    /// Asynchronously wait for user approval on proposed file diff
    pub async fn request_diff_approval(&self, subagent_id: &str, path: &str, diff: &str) -> bool {
        if self.is_paused.load(std::sync::atomic::Ordering::SeqCst) {
            return true;
        }
        if self.allowed_diffs.lock().unwrap().contains(path) {
            return true;
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = ConsoleRequest::DiffApproval {
            subagent_id: subagent_id.to_string(),
            path: path.to_string(),
            diff: diff.to_string(),
            responder: tx,
        };

        if self.sender.send(ConsoleEvent::Request(req)).is_err() {
            return false;
        }

        rx.await.unwrap_or(false)
    }

    /// Asynchronously read a line of input from terminal stdin through the unified reader
    pub async fn read_line(&self, prompt: &str) -> Option<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let req = ConsoleRequest::ReadLine {
            prompt: prompt.to_string(),
            responder: tx,
        };

        if self.sender.send(ConsoleEvent::Request(req)).is_err() {
            return None;
        }

        rx.await.unwrap_or(None)
    }
}

/// Coordinates dynamic subagent personas, instance lifecycles, and asynchronous execution.
pub struct SubagentManager {
    parent_id: String,
    base_config: AgentConfig,
    mailbox: Arc<MailboxManager>,
    worktree_mgr: Arc<WorktreeManager>,
    personas: Arc<RwLock<HashMap<String, SubagentPersona>>>,
    instances: Arc<RwLock<HashMap<String, SubagentInstance>>>,
    join_handles: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub approval_handler: Option<ApprovalHandler>,
    pub approval_queue: Option<Arc<SharedApprovalQueue>>,
}

impl SubagentManager {
    /// Initialize a new subagent manager with standard default personas registered.
    pub fn new(
        parent_id: impl Into<String>,
        base_config: AgentConfig,
        mailbox: Arc<MailboxManager>,
        worktree_mgr: Arc<WorktreeManager>,
    ) -> Self {
        let mut personas = HashMap::new();
        let r = SubagentPersona::researcher();
        let c = SubagentPersona::coder();
        let rev = SubagentPersona::reviewer();

        personas.insert(r.name.clone(), r);
        personas.insert(c.name.clone(), c);
        personas.insert(rev.name.clone(), rev);

        Self {
            parent_id: parent_id.into(),
            base_config,
            mailbox,
            worktree_mgr,
            personas: Arc::new(RwLock::new(personas)),
            instances: Arc::new(RwLock::new(HashMap::new())),
            join_handles: Arc::new(Mutex::new(HashMap::new())),
            approval_handler: None,
            approval_queue: None,
        }
    }

    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn with_approval_queue(mut self, queue: Arc<SharedApprovalQueue>) -> Self {
        self.approval_queue = Some(queue);
        self
    }

    /// Register or update a dynamic subagent persona.
    pub async fn register_persona(&self, persona: SubagentPersona) -> anyhow::Result<()> {
        let mut p = self.personas.write().await;
        p.insert(persona.name.clone(), persona);
        Ok(())
    }

    /// Get a registered persona by name.
    pub async fn get_persona(&self, name: &str) -> Option<SubagentPersona> {
        let p = self.personas.read().await;
        p.get(name).cloned()
    }

    /// List all currently registered subagent personas.
    pub async fn list_personas(&self) -> Vec<SubagentPersona> {
        let p = self.personas.read().await;
        p.values().cloned().collect()
    }

    /// Spawn a subagent instance using a registered persona name.
    pub async fn spawn_subagent(
        &self,
        persona_name: &str,
        prompt: &str,
        mode: WorkspaceMode,
    ) -> anyhow::Result<String> {
        let persona = self
            .get_persona(persona_name)
            .await
            .ok_or_else(|| anyhow::anyhow!("Subagent persona '{}' not registered", persona_name))?;

        self.spawn_custom_subagent(persona, prompt, mode).await
    }

    /// Spawn a subagent instance using an explicit persona configuration.
    pub async fn spawn_custom_subagent(
        &self,
        persona: SubagentPersona,
        prompt: &str,
        mode: WorkspaceMode,
    ) -> anyhow::Result<String> {
        let subagent_id = uuid::Uuid::new_v4().to_string();

        // 1. Create isolated workspace
        let isolated_ws = self
            .worktree_mgr
            .create_workspace(&subagent_id, mode)
            .await?;

        let instance = SubagentInstance {
            id: subagent_id.clone(),
            persona: persona.clone(),
            status: SubagentStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            workspace_dir: isolated_ws.path.clone(),
            workspace_mode: mode,
            result: None,
            error: None,
        };

        {
            let mut inst_map = self.instances.write().await;
            inst_map.insert(subagent_id.clone(), instance);
        }

        // 2. Clone base config for child agent
        let mut child_config = self.base_config.clone();
        child_config.workspace_dir = isolated_ws.path.clone();
        child_config.session_id = subagent_id.clone();
        child_config.model_tier = persona.model_tier;

        let instances = Arc::clone(&self.instances);
        let mailbox = Arc::clone(&self.mailbox);
        let id_clone = subagent_id.clone();
        let parent_id_clone = self.parent_id.clone();
        let prompt_owned = prompt.to_string();
        let approval_queue = self.approval_queue.clone();
        let approval_handler = self.approval_handler.clone();

        // 3. Spawn background execution task
        let handle = tokio::spawn(async move {
            // Update status to Running
            {
                let mut inst_map = instances.write().await;
                if let Some(inst) = inst_map.get_mut(&id_clone) {
                    inst.status = SubagentStatus::Running;
                    inst.updated_at = Utc::now();
                }
            }
            let _ = mailbox.notify_event(MailboxEvent::StatusChanged {
                agent_id: id_clone.clone(),
                status: "Running".to_string(),
            });

            // Initialize agent
            let agent_init = AgendexAgent::init(child_config).await;
            match agent_init {
                Ok(mut agent) => {
                    if let Some(queue) = &approval_queue {
                        agent = agent.with_approval_queue(Arc::clone(queue));
                    } else if let Some(handler) = &approval_handler {
                        agent = agent.with_approval_handler(Arc::clone(handler));
                    }
                    agent = agent
                        .with_allowed_tools(persona.allowed_tools.clone())
                        .with_mailbox(Arc::clone(&mailbox))
                        .with_agent_id(id_clone.clone())
                        .with_parent_id(parent_id_clone);

                    let turn_res = agent.process_turn(&prompt_owned).await;
                    match turn_res {
                        Ok(output) => {
                            let mut inst_map = instances.write().await;
                            if let Some(inst) = inst_map.get_mut(&id_clone) {
                                inst.status = SubagentStatus::Completed;
                                inst.result = Some(output.clone());
                                inst.updated_at = Utc::now();
                            }
                            let _ = mailbox.notify_event(MailboxEvent::TaskCompleted {
                                agent_id: id_clone.clone(),
                                result: output,
                            });
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            let mut inst_map = instances.write().await;
                            if let Some(inst) = inst_map.get_mut(&id_clone) {
                                inst.status = SubagentStatus::Failed;
                                inst.error = Some(err_msg.clone());
                                inst.updated_at = Utc::now();
                            }
                            let _ = mailbox.notify_event(MailboxEvent::TaskFailed {
                                agent_id: id_clone.clone(),
                                error: err_msg,
                            });
                        }
                    }
                }
                Err(e) => {
                    let err_msg = format!("Failed to initialize subagent agent: {}", e);
                    let mut inst_map = instances.write().await;
                    if let Some(inst) = inst_map.get_mut(&id_clone) {
                        inst.status = SubagentStatus::Failed;
                        inst.error = Some(err_msg.clone());
                        inst.updated_at = Utc::now();
                    }
                    let _ = mailbox.notify_event(MailboxEvent::TaskFailed {
                        agent_id: id_clone.clone(),
                        error: err_msg,
                    });
                }
            }
        });

        {
            let mut handles = self.join_handles.lock().await;
            handles.insert(subagent_id.clone(), handle);
        }

        Ok(subagent_id)
    }

    /// Retrieve an instance record by ID.
    pub async fn get_instance(&self, id: &str) -> Option<SubagentInstance> {
        let inst_map = self.instances.read().await;
        inst_map.get(id).cloned()
    }

    /// Retrieve the current status of an instance.
    pub async fn get_status(&self, id: &str) -> Option<SubagentStatus> {
        let inst_map = self.instances.read().await;
        inst_map.get(id).map(|i| i.status.clone())
    }

    /// List all subagent instances.
    pub async fn list_instances(&self) -> Vec<SubagentInstance> {
        let inst_map = self.instances.read().await;
        inst_map.values().cloned().collect()
    }

    /// Terminate a running subagent instance.
    pub async fn terminate_subagent(&self, id: &str) -> anyhow::Result<()> {
        {
            let mut handles = self.join_handles.lock().await;
            if let Some(handle) = handles.remove(id) {
                handle.abort();
            }
        }

        {
            let mut inst_map = self.instances.write().await;
            if let Some(inst) = inst_map.get_mut(id) {
                inst.status = SubagentStatus::Terminated;
                inst.updated_at = Utc::now();
            }
        }

        let _ = self.mailbox.notify_event(MailboxEvent::StatusChanged {
            agent_id: id.to_string(),
            status: "Terminated".to_string(),
        });

        Ok(())
    }

    /// Terminate all tracked subagents.
    pub async fn terminate_all(&self) -> anyhow::Result<()> {
        let ids: Vec<String> = {
            let inst_map = self.instances.read().await;
            inst_map.keys().cloned().collect()
        };

        for id in ids {
            let _ = self.terminate_subagent(&id).await;
        }

        Ok(())
    }

    /// Reactively wait until a subagent finishes without busy polling.
    pub async fn wait_for_subagent(
        &self,
        id: &str,
        timeout_dur: Option<Duration>,
    ) -> anyhow::Result<SubagentInstance> {
        // Fast path: if already finished, return immediately
        if let Some(inst) = self.get_instance(id).await {
            match inst.status {
                SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Terminated => {
                    return Ok(inst);
                }
                _ => {}
            }
        }

        // Subscribe to mailbox broadcast channel for reactive events
        let id_owned = id.to_string();
        let event = self
            .mailbox
            .wait_for_condition(
                move |evt| match evt {
                    MailboxEvent::TaskCompleted { agent_id, .. } => agent_id == &id_owned,
                    MailboxEvent::TaskFailed { agent_id, .. } => agent_id == &id_owned,
                    MailboxEvent::StatusChanged { agent_id, status } => {
                        agent_id == &id_owned
                            && (status == "Terminated"
                                || status == "Completed"
                                || status == "Failed")
                    }
                    _ => false,
                },
                timeout_dur,
            )
            .await?;

        tracing::debug!("Subagent {} finished with event {:?}", id, event);

        self.get_instance(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Subagent instance '{}' not found after completion", id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subagent_persona_registration() -> anyhow::Result<()> {
        let test_root =
            std::env::temp_dir().join(format!("agendex_swarm_persona_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&test_root)?;

        let mailbox = Arc::new(MailboxManager::new());
        let worktree_mgr = Arc::new(WorktreeManager::new(test_root.clone(), None)?);
        let base_config = AgentConfig {
            workspace_dir: test_root.clone(),
            ..Default::default()
        };

        let manager = SubagentManager::new("parent", base_config, mailbox, worktree_mgr);

        // Check defaults
        assert!(manager.get_persona("researcher").await.is_some());
        assert!(manager.get_persona("coder").await.is_some());
        assert!(manager.get_persona("reviewer").await.is_some());

        // Register custom persona
        let custom = SubagentPersona::new(
            "security_auditor",
            "Auditor",
            "Perform security scans",
            vec!["read_file".to_string()],
            ModelTier::Standard128k,
        );
        manager.register_persona(custom).await?;

        assert!(manager.get_persona("security_auditor").await.is_some());

        // Approval handler wiring check
        let custom_mgr = manager.with_approval_handler(Arc::new(|_cmd, _reason| true));
        assert!(custom_mgr.approval_handler.is_some());

        let _ = std::fs::remove_dir_all(&test_root);
        Ok(())
    }
}
