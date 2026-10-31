use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use agendex_ast::patcher::AstPatcher;
use agendex_harness::tasks::TaskManager;
use agendex_harness::tools::ToolRegistry;
use agendex_mcp::McpRegistry;
use agendex_scaffold::hooks::HookDispatcher;
use agendex_scaffold::rules::RulesEngine;
use agendex_scaffold::skills::SkillRegistry;
use agendex_trajectory::compactor::{CompactorEngine, ModelTier};
use agendex_trajectory::snapshots::SnapshotEngine;
use agendex_trajectory::trajectory::TrajectoryLogger;

use crate::model::{ChatMessage, ModelClient, ModelConfig, ToolCall};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub workspace_dir: PathBuf,
    pub model_tier: ModelTier,
    pub auto_compaction: bool,
    pub session_id: String,
    pub model: ModelConfig,
    pub max_tool_iterations: usize,
    pub mcp_config: Option<PathBuf>,
    pub auto_approve_risky: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            workspace_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            model_tier: ModelTier::Standard128k,
            auto_compaction: true,
            session_id: uuid::Uuid::new_v4().to_string(),
            model: ModelConfig::default(),
            max_tool_iterations: 25,
            mcp_config: None,
            auto_approve_risky: false,
        }
    }
}

pub type ApprovalHandler = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
pub type DiffApprovalHandler = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;
pub type ToolProgressHandler = Arc<dyn Fn(&str) + Send + Sync>;
pub type TokenStreamHandler = Arc<dyn Fn(&str) + Send + Sync>;

pub struct AgendexAgent {
    pub config: AgentConfig,
    pub model_client: Arc<tokio::sync::RwLock<ModelClient>>,
    pub active_model_config: Arc<tokio::sync::RwLock<crate::model::ModelConfig>>,
    pub active_provider_name: Arc<tokio::sync::RwLock<String>>,
    pub task_manager: Arc<TaskManager>,
    pub tool_registry: Arc<ToolRegistry>,
    pub trajectory: Arc<Mutex<TrajectoryLogger>>,
    pub compactor: Arc<Mutex<CompactorEngine>>,
    pub snapshot_engine: Arc<Mutex<SnapshotEngine>>,
    pub skill_registry: Arc<std::sync::RwLock<SkillRegistry>>,
    pub rules_engine: Arc<RulesEngine>,
    pub hook_dispatcher: Arc<HookDispatcher>,
    pub ast_patcher: Arc<AstPatcher>,
    pub mcp_registry: Arc<McpRegistry>,
    pub mailbox: Option<Arc<crate::mailbox::MailboxManager>>,
    pub allowed_tools: Option<Vec<String>>,
    pub agent_id: Option<String>,
    pub parent_id: Option<String>,
    pub history: Arc<Mutex<Vec<ChatMessage>>>,
    pub approval_handler: Option<ApprovalHandler>,
    pub diff_approval_handler: Option<DiffApprovalHandler>,
    pub approval_queue: Option<Arc<crate::swarm::SharedApprovalQueue>>,
    pub event_handler: Option<crate::events::AgentEventHandler>,
    pub tool_progress_handler: Option<ToolProgressHandler>,
    pub token_stream_handler: Option<TokenStreamHandler>,
    pub activity_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    pub touched_files: Arc<Mutex<std::collections::HashSet<String>>>,
    pub streamed_tokens_in_turn: Arc<std::sync::atomic::AtomicBool>,
    pub auto_approve_yolo: Arc<std::sync::atomic::AtomicBool>,
    pub current_turn_prompt_tokens: Arc<std::sync::atomic::AtomicUsize>,
    pub current_turn_completion_tokens: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffApprovalResult {
    Approved,
    RejectedByUser,
    RejectedNonTerminal,
}

impl AgendexAgent {
    pub async fn init(config: AgentConfig) -> anyhow::Result<Self> {
        // Ensure environment variables from .env are loaded into the process
        crate::config::load_agendex_env();

        // Initialize SnapshotEngine first (ensures git and state_dir are initialized consistently)
        let snapshot_engine_raw = SnapshotEngine::new(&config.workspace_dir)?;
        let state_dir = snapshot_engine_raw.state_dir().to_path_buf();
        let log_dir = state_dir.join("logs").join(&config.session_id);
        tokio::fs::create_dir_all(&log_dir).await?;

        let task_manager = Arc::new(TaskManager::new(state_dir.join("tasks")));
        let tool_registry = Arc::new(
            ToolRegistry::new(config.workspace_dir.clone(), Arc::clone(&task_manager))
                .with_logs_dir(state_dir.join("logs")),
        );
        let trajectory = Arc::new(Mutex::new(TrajectoryLogger::new(&log_dir)?));
        let compactor = Arc::new(Mutex::new(CompactorEngine::new(config.model_tier)));
        let snapshot_engine = Arc::new(Mutex::new(snapshot_engine_raw));

        let mut skill_registry = SkillRegistry::new();
        let _ = skill_registry.discover_skills(&config.workspace_dir).await;

        let mut rules_engine = RulesEngine::new();
        let _ = rules_engine.load_rules(&config.workspace_dir).await;

        let hook_dispatcher = Arc::new(HookDispatcher::new(&config.workspace_dir).await?);
        let ast_patcher = Arc::new(AstPatcher::new());
        let mcp_registry = Arc::new(McpRegistry::new());
        let active_model_config = Arc::new(tokio::sync::RwLock::new(config.model.clone()));
        let active_provider_name = Arc::new(tokio::sync::RwLock::new(
            if let Some(cfg) = crate::config::load_default_config() {
                cfg.active_provider.unwrap_or_else(|| "nim".to_string())
            } else {
                "nim".to_string()
            },
        ));
        let model_client = Arc::new(tokio::sync::RwLock::new(ModelClient::new(
            config.model.clone(),
        )));

        // Discover and load MCP configuration if specified or present in workspace
        let mcp_path = config.mcp_config.clone().or_else(|| {
            let p1 = config.workspace_dir.join(".agendex").join("mcp.json");
            if p1.exists() {
                Some(p1)
            } else {
                let p2 = config.workspace_dir.join(".agenda").join("mcp.json");
                if p2.exists() {
                    Some(p2)
                } else {
                    None
                }
            }
        });

        if let Some(cfg_path) = mcp_path {
            match mcp_registry.load_config_file(&cfg_path).await {
                Ok(count) => {
                    tracing::info!("Loaded {} MCP tools from {}", count, cfg_path.display());
                }
                Err(e) => {
                    tracing::warn!("Failed loading MCP config {}: {}", cfg_path.display(), e);
                }
            }
        }

        let system_prompt = format!(
            "You are Agendex, a powerful local-first autonomous agent.\n\
            Workspace: {}\n\
            Available Slash Commands:\n{}\n\
            Active Rules:\n{}",
            config.workspace_dir.display(),
            skill_registry.format_slash_commands_prompt(),
            rules_engine.generate_rule_blocks(None)
        );

        // Restore conversation history from logged trajectory steps if resuming
        let history = Arc::new(Mutex::new(Vec::new()));
        {
            let mut hist = history.lock().await;
            hist.push(ChatMessage::system(system_prompt));

            let traj = trajectory.lock().await;
            let steps = traj.get_steps();
            for step in steps {
                match step.step_type.as_str() {
                    "USER_INPUT" => {
                        hist.push(ChatMessage::user(&step.content));
                    }
                    "PLANNER_RESPONSE" | "ASSISTANT" => {
                        let tcs = step.tool_calls.as_ref().and_then(|val| {
                            serde_json::from_value::<Vec<ToolCall>>(val.clone()).ok()
                        });
                        hist.push(ChatMessage::assistant(Some(step.content.clone()), tcs));
                    }
                    "TOOL_RESULT" => {
                        let tid = step
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("id"))
                            .or_else(|| step.metadata.as_ref().and_then(|m| m.get("tool_call_id")))
                            .and_then(|id| id.as_str())
                            .unwrap_or("tool_call");
                        let tname = step
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("tool"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("tool");
                        hist.push(ChatMessage::tool_result(tid, tname, &step.content));
                    }
                    "COMPACTION" => {
                        hist.push(ChatMessage::system(format!(
                            "[COMPACTED STATE]:\n{}",
                            step.content
                        )));
                    }
                    _ => {}
                }
            }
        }

        Ok(Self {
            config: config.clone(),
            model_client,
            active_model_config,
            active_provider_name,
            task_manager,
            tool_registry,
            trajectory,
            compactor,
            snapshot_engine,
            skill_registry: Arc::new(std::sync::RwLock::new(skill_registry)),
            rules_engine: Arc::new(rules_engine),
            hook_dispatcher,
            ast_patcher,
            mcp_registry,
            mailbox: None,
            allowed_tools: None,
            agent_id: Some(config.session_id),
            parent_id: None,
            history,
            approval_handler: None,
            diff_approval_handler: None,
            approval_queue: None,
            event_handler: None,
            tool_progress_handler: None,
            token_stream_handler: None,
            activity_hook: None,
            touched_files: Arc::new(Mutex::new(std::collections::HashSet::new())),
            streamed_tokens_in_turn: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            auto_approve_yolo: Arc::new(std::sync::atomic::AtomicBool::new(
                config.auto_approve_risky,
            )),
            current_turn_prompt_tokens: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            current_turn_completion_tokens: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    pub fn streamed_tokens_in_last_turn(&self) -> bool {
        self.streamed_tokens_in_turn
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Records token consumption for a user-interrupted turn (Ctrl-C) so session cost accounting remains accurate.
    pub async fn record_interrupted_turn(&self) {
        let p_tok = self
            .current_turn_prompt_tokens
            .load(std::sync::atomic::Ordering::SeqCst);
        let c_tok = self
            .current_turn_completion_tokens
            .load(std::sync::atomic::Ordering::SeqCst);
        if p_tok == 0 && c_tok == 0 {
            return;
        }

        let model_name = self.config.model.model_name.clone();
        let has_known = agendex_trajectory::is_known_pricing_model(&model_name);
        let est_cost = agendex_trajectory::estimate_cost(&model_name, p_tok, c_tok);

        let mut traj = self.trajectory.lock().await;
        let _ = traj
            .log_step(
                "INTERRUPTED",
                "USER",
                "Turn interrupted by user (Ctrl-C)",
                serde_json::json!({ "prompt_tokens": p_tok, "completion_tokens": c_tok }),
            )
            .await;
        let _ = traj.record_token_usage(None, p_tok, c_tok, est_cost, has_known);

        self.current_turn_prompt_tokens
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.current_turn_completion_tokens
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn with_diff_approval_handler(mut self, handler: DiffApprovalHandler) -> Self {
        self.diff_approval_handler = Some(handler);
        self
    }

    pub fn with_tool_progress_handler(mut self, handler: ToolProgressHandler) -> Self {
        self.tool_progress_handler = Some(handler);
        self
    }

    pub fn with_token_stream_handler(mut self, handler: TokenStreamHandler) -> Self {
        self.token_stream_handler = Some(handler);
        self
    }

    pub fn with_activity_hook(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.activity_hook = Some(hook);
        self
    }

    pub fn notify_activity(&self) {
        if let Some(hook) = &self.activity_hook {
            hook();
        }
    }

    pub fn notify_progress(&self, msg: &str) {
        self.notify_activity();
        if let Some(handler) = &self.tool_progress_handler {
            let masked = agendex_trajectory::mask_secrets(msg);
            handler(&masked);
        }
    }

    pub async fn request_diff_approval(&self, path: &str, diff: &str) -> DiffApprovalResult {
        self.notify_activity();
        if self.config.auto_approve_risky
            || self
                .auto_approve_yolo
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return DiffApprovalResult::Approved;
        }
        if let Some(handler) = &self.diff_approval_handler {
            let masked_diff = agendex_trajectory::mask_secrets(diff);
            return if handler(path, &masked_diff) {
                DiffApprovalResult::Approved
            } else {
                DiffApprovalResult::RejectedByUser
            };
        }
        if let Some(queue) = &self.approval_queue {
            let agent_id = self.agent_id.as_deref().unwrap_or("parent");
            return if queue.request_diff_approval(agent_id, path, diff).await {
                DiffApprovalResult::Approved
            } else {
                DiffApprovalResult::RejectedByUser
            };
        }
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            use std::io::Write;
            println!("\n[Diff Review] Proposed changes to '{}':", path);
            println!("{}", diff);
            print!("Apply this change? [y/N]: ");
            let _ = std::io::stdout().flush();
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
                if trimmed == "y" || trimmed == "yes" {
                    return DiffApprovalResult::Approved;
                }
            }
            return DiffApprovalResult::RejectedByUser;
        }
        // Non-terminal execution without -y refuses edits, matching risky command policy
        DiffApprovalResult::RejectedNonTerminal
    }

    pub fn with_event_handler(mut self, handler: crate::events::AgentEventHandler) -> Self {
        self.event_handler = Some(handler);
        self
    }

    pub fn emit_event(&self, event: crate::events::AgentEvent) {
        if let Some(handler) = &self.event_handler {
            handler(&event);
        }
    }

    /// Explicitly records a file path as touched by the agent during this session.
    pub async fn record_touched_file(&self, path: impl AsRef<std::path::Path>) {
        let p_str = path.as_ref().to_string_lossy();
        let norm = agendex_trajectory::normalize_rel_path(&p_str, &self.config.workspace_dir);
        self.touched_files.lock().await.insert(norm);
    }

    pub async fn commit_workspace_changes(
        &self,
        summary: &str,
    ) -> anyhow::Result<Option<crate::git::GitCommitResult>> {
        let mut touched = {
            let files = self.touched_files.lock().await;
            files.iter().cloned().collect::<Vec<_>>()
        };
        {
            let snap = self.snapshot_engine.lock().await;
            for f in snap.all_agent_created_files() {
                if !touched.contains(&f) {
                    touched.push(f);
                }
            }
        }
        let target_files = if !touched.is_empty() {
            Some(touched)
        } else {
            None
        };
        let res = crate::git::commit_task_changes(
            &self.config.workspace_dir,
            summary,
            target_files.as_deref(),
        )
        .await?;
        if let Some(ref c) = res {
            self.emit_event(crate::events::AgentEvent::GitCommit {
                commit_sha: c.commit_sha.clone(),
                message: c.message.clone(),
            });
        }
        Ok(res)
    }

    pub async fn switch_or_create_branch(&self, branch: &str) -> anyhow::Result<String> {
        let res = crate::git::create_or_checkout_branch(&self.config.workspace_dir, branch).await?;
        self.emit_event(crate::events::AgentEvent::GitBranch {
            branch: branch.to_string(),
        });
        Ok(res)
    }

    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    pub fn with_approval_queue(mut self, queue: Arc<crate::swarm::SharedApprovalQueue>) -> Self {
        self.approval_queue = Some(queue);
        self
    }

    pub fn with_auto_approve(mut self, auto: bool) -> Self {
        self.config.auto_approve_risky = auto;
        self
    }

    pub fn with_mailbox(mut self, mailbox: Arc<crate::mailbox::MailboxManager>) -> Self {
        self.mailbox = Some(mailbox);
        self
    }

    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_parent_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_model_client(
        mut self,
        model_client: Arc<tokio::sync::RwLock<ModelClient>>,
    ) -> Self {
        self.model_client = model_client;
        self
    }

    /// Create a subagent manager inheriting this agent's configuration, mailbox, and approval handler.
    pub fn create_subagent_manager(
        &self,
        worktree_mgr: Arc<crate::worktree::WorktreeManager>,
    ) -> crate::swarm::SubagentManager {
        let parent_id = self
            .agent_id
            .clone()
            .unwrap_or_else(|| self.config.session_id.clone());
        let mailbox = self
            .mailbox
            .clone()
            .unwrap_or_else(|| Arc::new(crate::mailbox::MailboxManager::new()));
        let mut mgr = crate::swarm::SubagentManager::new(
            parent_id,
            self.config.clone(),
            mailbox,
            worktree_mgr,
        );
        if let Some(queue) = &self.approval_queue {
            mgr = mgr.with_approval_queue(Arc::clone(queue));
        } else if let Some(handler) = &self.approval_handler {
            mgr = mgr.with_approval_handler(Arc::clone(handler));
        }
        mgr
    }

    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        match &self.allowed_tools {
            Some(allowed) => allowed.iter().any(|t| t == tool_name),
            None => true,
        }
    }

    pub async fn process_turn(&self, user_input: &str) -> anyhow::Result<String> {
        match self.process_turn_inner(user_input).await {
            Ok(res) => Ok(res),
            Err(e) => {
                self.emit_event(crate::events::AgentEvent::Error {
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }

    async fn process_turn_inner(&self, user_input: &str) -> anyhow::Result<String> {
        self.touched_files.lock().await.clear();
        self.streamed_tokens_in_turn
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.emit_event(crate::events::AgentEvent::TurnStart {
            session_id: self.config.session_id.clone(),
            prompt: agendex_trajectory::mask_secrets(user_input),
        });

        // 1. Check for slash command resolution
        if user_input.starts_with('/') {
            let mut parts = user_input.split_whitespace();
            let cmd = parts.next().unwrap_or("");
            let rest_args = user_input.strip_prefix(cmd).unwrap_or("").trim();
            match cmd {
                "/help" => {
                    return Ok(self.handle_help_command());
                }
                "/cost" => {
                    return Ok(self.handle_cost_command().await);
                }
                "/tokens" => {
                    return Ok(self.handle_tokens_command().await);
                }
                "/context" => {
                    return Ok(self.handle_context_command().await);
                }
                "/diff" => {
                    return self.handle_diff_command().await;
                }
                "/skills" => {
                    return Ok(self.handle_skills_command());
                }
                "/mcp" => {
                    return Ok(self.handle_mcp_command().await);
                }
                "/tools" => {
                    return Ok(self.handle_tools_command().await);
                }
                "/init" => {
                    return self.handle_init_command().await;
                }
                "/config" => {
                    return Ok(self.handle_config_command());
                }
                "/yolo" => {
                    let cur = self
                        .auto_approve_yolo
                        .load(std::sync::atomic::Ordering::SeqCst);
                    let nxt = !cur;
                    self.auto_approve_yolo
                        .store(nxt, std::sync::atomic::Ordering::SeqCst);
                    if nxt {
                        return Ok("⚡ YOLO mode ENABLED: commands and file edits will execute without interactive approval prompts.".to_string());
                    } else {
                        return Ok("🛡️ YOLO mode DISABLED: requiring interactive approval for commands and edits.".to_string());
                    }
                }
                "/new" => {
                    return self.handle_new_command().await;
                }
                "/resume" => {
                    return self.handle_resume_command(rest_args).await;
                }
                "/compact" => {
                    let mut comp = self.compactor.lock().await;
                    let mut history = self.history.lock().await;
                    let before_count = history.len();
                    comp.grant_immunity(1);

                    let safe_cut = find_safe_compaction_boundary(&history, 4);
                    let evicted_slice: Vec<ChatMessage> =
                        if safe_cut > 2 && safe_cut < history.len() {
                            history[1..safe_cut].to_vec()
                        } else {
                            Vec::new()
                        };

                    let mut summary = String::new();
                    if !evicted_slice.is_empty() {
                        // 1. Try real model-generated continuation summary
                        let mut prompt_msgs = vec![
                            ChatMessage::system(
                                "You are an autonomous context compactor. Summarize the following evicted conversation turns into a concise, factual continuation summary.\n\
                                Format your response with exactly three sections:\n\
                                ### 1. Task Overview & Initial Objectives\n\
                                - Core objectives and requirements requested by the user\n\
                                ### 2. Key Decisions & Structural Changes Made\n\
                                - Concrete files created or modified, tools called, and decisions\n\
                                ### 3. Current State & Next Steps\n\
                                - Active status of the codebase and immediate remaining tasks"
                            ),
                        ];
                        prompt_msgs.extend(evicted_slice.iter().cloned());
                        prompt_msgs.push(ChatMessage::user(
                            "Generate the structured continuation summary for the above turns now.",
                        ));

                        if let Ok(client) = self.model_client.try_read() {
                            if let Ok(resp) = client.chat(&prompt_msgs, &[]).await {
                                if let Some(txt) = resp.text {
                                    if !txt.trim().is_empty() {
                                        summary = txt.trim().to_string();
                                    }
                                }
                            }
                        }

                        // 2. Dynamic fallback: extract user requests and tool execution deltas
                        if summary.is_empty() {
                            let mut user_goals = Vec::new();
                            let mut tools_used = Vec::new();
                            let mut final_notes = Vec::new();

                            for m in &evicted_slice {
                                if m.role == "user" && !m.content.starts_with('/') {
                                    let first_line = m.content.lines().next().unwrap_or("").trim();
                                    if !first_line.is_empty() && user_goals.len() < 3 {
                                        user_goals.push(first_line.to_string());
                                    }
                                } else if m.role == "assistant" {
                                    if let Some(ref tcs) = m.tool_calls {
                                        for tc in tcs {
                                            let note = match tc.name.as_str() {
                                                "write_file" | "edit_file" => {
                                                    let path = tc
                                                        .arguments
                                                        .get("path")
                                                        .and_then(|p| p.as_str())
                                                        .unwrap_or("");
                                                    format!("{}: {}", tc.name, path)
                                                }
                                                "run_command" => {
                                                    let cmd = tc
                                                        .arguments
                                                        .get("command_line")
                                                        .and_then(|c| c.as_str())
                                                        .unwrap_or("");
                                                    let short_cmd = cmd
                                                        .split_whitespace()
                                                        .take(4)
                                                        .collect::<Vec<_>>()
                                                        .join(" ");
                                                    format!("run_command: {}", short_cmd)
                                                }
                                                _ => tc.name.clone(),
                                            };
                                            if !tools_used.contains(&note) && tools_used.len() < 6 {
                                                tools_used.push(note);
                                            }
                                        }
                                    } else if !m.content.is_empty() && final_notes.len() < 2 {
                                        let first_line =
                                            m.content.lines().next().unwrap_or("").trim();
                                        if !first_line.is_empty() {
                                            final_notes.push(first_line.to_string());
                                        }
                                    }
                                }
                            }

                            let mut s = String::new();
                            s.push_str("### 1. Task Overview & Initial Objectives\n");
                            if user_goals.is_empty() {
                                s.push_str("- Active autonomous software engineering session.\n");
                            } else {
                                for g in &user_goals {
                                    s.push_str(&format!("- Goal: {}\n", g));
                                }
                            }
                            s.push_str("\n### 2. Key Decisions & Structural Changes Made\n");
                            if tools_used.is_empty() {
                                s.push_str("- Initial repository inspection and baseline setup.\n");
                            } else {
                                for t in &tools_used {
                                    s.push_str(&format!("- Executed {}\n", t));
                                }
                            }
                            s.push_str("\n### 3. Current State & Next Steps\n");
                            if final_notes.is_empty() {
                                s.push_str(
                                    "- Working tree verified; ready for subsequent instructions.\n",
                                );
                            } else {
                                for n in &final_notes {
                                    s.push_str(&format!("- State: {}\n", n));
                                }
                            }
                            summary = s;
                        }
                    } else {
                        summary = comp.generate_continuation_summary(if rest_args.is_empty() {
                            "Manual context compaction triggered."
                        } else {
                            rest_args
                        });
                    }

                    let evicted_count = if safe_cut > 2 && safe_cut < history.len() {
                        let mut compacted = Vec::new();
                        compacted.push(history[0].clone()); // system prompt
                        if history.len() > 1 {
                            compacted.push(history[1].clone()); // initial user prompt
                        }
                        compacted.push(ChatMessage::system(format!(
                            "[Context Compaction Anchor]\n{}",
                            summary
                        )));
                        compacted.extend_from_slice(&history[safe_cut..]);
                        let evicted = history.len().saturating_sub(compacted.len());
                        *history = compacted;
                        evicted
                    } else {
                        0
                    };

                    return Ok(format!(
                        "Context compacted successfully.\n• Messages before: {}\n• Messages retained: {}\n• Evicted: {}\n\nContinuation Anchor:\n{}",
                        before_count,
                        history.len(),
                        evicted_count,
                        summary
                    ));
                }
                "/def" | "/definition" => {
                    let sym = rest_args;
                    if sym.is_empty() {
                        return Ok("Usage: /def <symbol_name>".to_string());
                    }
                    let res = self
                        .tool_registry
                        .find_definition(agendex_harness::tools::FindDefinitionArgs {
                            symbol: sym.to_string(),
                            path: None,
                            kind: None,
                        })
                        .await?;
                    if res.definitions.is_empty() {
                        return Ok(format!("No definitions found for symbol '{}'", sym));
                    }
                    let mut out =
                        format!("Found {} definition(s) for '{}':\n", res.total_found, sym);
                    for d in &res.definitions {
                        out.push_str(&format!(
                            "- [{}] {} ({}:{})\n  Preview: {}\n",
                            d.kind,
                            d.symbol,
                            d.path.display(),
                            d.line,
                            d.preview
                        ));
                    }
                    return Ok(out);
                }
                "/undo" | "/revert" => {
                    let mut snap = self.snapshot_engine.lock().await;
                    match snap.rollback_turn().await {
                        Ok(Some(res)) => {
                            if let Some(step_idx) = res.step_index {
                                let mut traj = self.trajectory.lock().await;
                                let _ = traj.truncate_after(step_idx.saturating_sub(1));
                            }
                            let msg = if res.removed_files.is_empty() {
                                format!(
                                    "Reverted workspace to checkpoint [{}] (all changes in turn undone).",
                                    res.snapshot.label
                                )
                            } else {
                                format!(
                                    "Reverted workspace to checkpoint [{}] (undone turn, removed {} agent-created file(s)).",
                                    res.snapshot.label,
                                    res.removed_files.len()
                                )
                            };
                            return Ok(msg);
                        }
                        Ok(None) => {
                            return Ok("No previous turn snapshot available to revert.".to_string());
                        }
                        Err(err) => {
                            return Ok(format!("[Refused] {}", err));
                        }
                    }
                }
                "/provider" | "/providers" | "/model" | "/models" => {
                    return self.handle_provider_command(user_input).await;
                }
                _ => {
                    let matched_skill = self
                        .skill_registry
                        .read()
                        .ok()
                        .and_then(|r| r.get_skill_by_command(cmd).cloned());
                    if let Some(skill) = matched_skill {
                        tracing::info!("Dispatched dynamic skill command: {}", cmd);
                        return Ok(format!(
                            "Skill [{}]:\n{}\n\nProtocol Instructions:\n{}",
                            skill.name, skill.description, skill.content
                        ));
                    }
                }
            }
        }

        // 2. Pre-turn hooks
        self.hook_dispatcher.trigger_pre_turn(user_input).await?;

        // 3. Workspace Snapshot before turn
        let start_step_count = { self.trajectory.lock().await.step_count() };
        let turn_step_index = (start_step_count + 1) as u64;
        let turn_label = format!("turn_start:{}", turn_step_index);
        {
            let mut snap = self.snapshot_engine.lock().await;
            let _ = snap
                .capture_snapshot_with_metadata(
                    &turn_label,
                    Some(serde_json::json!({
                        "kind": "turn_start",
                        "step_index": turn_step_index,
                        "created_files": []
                    })),
                )
                .await;
        }

        // 4. Record User Input into Trajectory
        {
            let mut traj = self.trajectory.lock().await;
            traj.log_step("USER_INPUT", "USER", user_input, serde_json::json!({}))
                .await?;
        }

        // 5. Gather Tool Definitions
        let mut tools = Vec::new();
        for tool_def in self.tool_registry.list_tool_definitions() {
            let name = tool_def.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if self.is_tool_allowed(name) {
                tools.push(tool_def);
            }
        }

        // Add MCP tools from the registry
        for mcp_tool in self.mcp_registry.list_tools().await {
            if self.is_tool_allowed(&mcp_tool.name) {
                tools.push(serde_json::json!({
                    "name": mcp_tool.name,
                    "description": mcp_tool.description.unwrap_or_default(),
                    "parameters": mcp_tool.input_schema,
                }));
            }
        }

        // Add edit_file tool (AST patcher) if permitted
        if self.is_tool_allowed("edit_file") {
            tools.push(serde_json::json!({
                "name": "edit_file",
                "description": "Surgically replace contiguous code blocks matching exact target text and leading indentation",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute or workspace-relative path" },
                        "target": { "type": "string", "description": "Exact character sequence to be replaced" },
                        "replacement": { "type": "string", "description": "Replacement string; leave empty to delete" }
                    },
                    "required": ["path", "target", "replacement"]
                }
            }));
        }

        // Add send_message tool if reactive mailbox is present and permitted
        if self.mailbox.is_some() && self.is_tool_allowed("send_message") {
            tools.push(serde_json::json!({
                "name": "send_message",
                "description": "Send an asynchronous message to another agent or parent via the reactive mailbox",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "recipient": { "type": "string", "description": "Recipient agent ID or 'parent'" },
                        "message": { "type": "string", "description": "Message payload content" }
                    },
                    "required": ["recipient", "message"]
                }
            }));
        }

        // Add save_skill tool if permitted
        if self.is_tool_allowed("save_skill") {
            tools.push(agendex_scaffold::save_skill_tool_definition());
        }

        // 6. Access and update conversation history across turns
        let mut history = self.history.lock().await;

        if history.is_empty() {
            let slash_prompt = self
                .skill_registry
                .read()
                .map(|r| r.format_slash_commands_prompt())
                .unwrap_or_default();
            let system_prompt = format!(
                "You are Agendex, a powerful local-first autonomous agent.\n\
                Workspace: {}\n\
                Available Slash Commands:\n{}\n\
                Active Rules:\n{}",
                self.config.workspace_dir.display(),
                slash_prompt,
                self.rules_engine.generate_rule_blocks(None)
            );
            history.push(ChatMessage::system(system_prompt));
        }

        history.push(ChatMessage::user(user_input));

        let mut turn_iteration = 0;
        let mut total_tokens = 0;
        let mut turn_prompt_tokens = 0;
        let mut turn_completion_tokens = 0;

        // 7. Multi-Turn Model and Tool Execution Loop
        while turn_iteration < self.config.max_tool_iterations {
            turn_iteration += 1;

            // Check auto-compaction threshold
            if self.config.auto_compaction {
                let mut comp = self.compactor.lock().await;
                if comp.should_compact(total_tokens) {
                    let summary =
                        comp.generate_continuation_summary("Active execution in progress.");
                    tracing::info!("Auto-compaction triggered at {} tokens.", total_tokens);
                    let mut traj = self.trajectory.lock().await;
                    let _ = traj
                        .log_step(
                            "COMPACTION",
                            "SYSTEM",
                            &summary,
                            serde_json::json!({"total_tokens": total_tokens}),
                        )
                        .await;

                    // Compact in-memory message history to head and tail cleanly without breaking tool pairs
                    if history.len() > 6 {
                        let safe_cut = find_safe_compaction_boundary(&history, 4);
                        if safe_cut > 2 && safe_cut < history.len() {
                            let mut compacted = Vec::new();
                            compacted.push(history[0].clone()); // system prompt
                            compacted.push(history[1].clone()); // initial user objective
                            compacted.push(ChatMessage::system(format!(
                                "[COMPACTED STATE]:\n{}",
                                summary
                            )));
                            compacted.extend_from_slice(&history[safe_cut..]);
                            *history = compacted;
                        }
                    }
                }
            }

            // Call Model
            self.notify_progress("Generating response...");
            let est_prompt = (history.iter().map(|m| m.content.len()).sum::<usize>() + 2000) / 4;
            self.current_turn_prompt_tokens
                .store(est_prompt, std::sync::atomic::Ordering::SeqCst);

            let stream_handler: Option<TokenStreamHandler> =
                if let Some(handler) = &self.token_stream_handler {
                    let flag = Arc::clone(&self.streamed_tokens_in_turn);
                    let h = Arc::clone(handler);
                    let comp_counter = Arc::clone(&self.current_turn_completion_tokens);
                    Some(Arc::new(move |token: &str| {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        comp_counter.fetch_add(
                            (token.len() / 4).max(1),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        let masked_token = agendex_trajectory::mask_secrets(token);
                        h(&masked_token);
                    }))
                } else {
                    None
                };
            let resp = {
                let client = self.model_client.read().await;
                client
                    .chat_with_stream(&history, &tools, stream_handler)
                    .await?
            };

            turn_prompt_tokens += resp.prompt_tokens;
            turn_completion_tokens += resp.completion_tokens;
            total_tokens = resp.prompt_tokens + resp.completion_tokens;
            self.current_turn_prompt_tokens
                .store(turn_prompt_tokens, std::sync::atomic::Ordering::SeqCst);
            self.current_turn_completion_tokens
                .store(turn_completion_tokens, std::sync::atomic::Ordering::SeqCst);

            // If the model emitted tool calls, execute them
            if !resp.tool_calls.is_empty() {
                self.notify_progress(&format!(
                    "Model requested {} tool call(s)",
                    resp.tool_calls.len()
                ));
                history.push(ChatMessage::assistant(
                    resp.text.clone(),
                    Some(resp.tool_calls.clone()),
                ));

                // Log the assistant tool-call step to Trajectory Engine
                {
                    let mut traj = self.trajectory.lock().await;
                    let tc_val = serde_json::to_value(&resp.tool_calls).ok();
                    let _ = traj
                        .log_step_detailed(agendex_trajectory::TrajectoryStepInput {
                            step_type: "ASSISTANT".to_string(),
                            source: "MODEL".to_string(),
                            status: "DONE".to_string(),
                            content: resp.text.clone().unwrap_or_default(),
                            thinking: None,
                            tool_calls: tc_val,
                            media: None,
                            metadata: None,
                        })
                        .await;
                }

                for tc in &resp.tool_calls {
                    self.emit_event(crate::events::AgentEvent::ToolCall {
                        id: tc.id.clone(),
                        tool: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    });
                }

                for tc in resp.tool_calls {
                    let tool_result = self.dispatch_tool(&tc).await;

                    let res_val = serde_json::from_str(&tool_result)
                        .unwrap_or_else(|_| serde_json::Value::String(tool_result.clone()));
                    self.emit_event(crate::events::AgentEvent::ToolResult {
                        id: tc.id.clone(),
                        tool: tc.name.clone(),
                        result: res_val,
                    });

                    // Log to Trajectory Engine
                    {
                        let mut traj = self.trajectory.lock().await;
                        let _ = traj
                            .log_step(
                                "TOOL_RESULT",
                                "SYSTEM",
                                &tool_result,
                                serde_json::json!({
                                    "tool": tc.name,
                                    "id": tc.id,
                                    "tool_name": tc.name,
                                    "call_id": tc.id
                                }),
                            )
                            .await;
                    }

                    // Append tool response
                    history.push(ChatMessage::tool_result(&tc.id, &tc.name, tool_result));
                }
            } else {
                // Final textual response received
                let final_text = resp.text.unwrap_or_else(|| "Completed turn.".to_string());
                self.emit_event(crate::events::AgentEvent::FinalOutput {
                    text: final_text.clone(),
                });
                history.push(ChatMessage::assistant(Some(final_text.clone()), None));
                {
                    let mut traj = self.trajectory.lock().await;
                    let _ = traj
                        .log_step(
                            "PLANNER_RESPONSE",
                            "MODEL",
                            &final_text,
                            serde_json::json!({}),
                        )
                        .await;
                }

                // Record turn token consumption & cost accounting in Trajectory
                let has_known =
                    agendex_trajectory::is_known_pricing_model(&self.config.model.model_name);
                let est_cost = agendex_trajectory::estimate_cost(
                    &self.config.model.model_name,
                    turn_prompt_tokens,
                    turn_completion_tokens,
                );
                {
                    let mut traj = self.trajectory.lock().await;
                    let _ = traj.record_token_usage(
                        Some(turn_step_index),
                        turn_prompt_tokens,
                        turn_completion_tokens,
                        est_cost,
                        has_known,
                    );
                }

                self.hook_dispatcher
                    .trigger_post_tool("turn_complete", "success")
                    .await?;

                // Record turn completion in SnapshotEngine to preserve post-turn tree state
                {
                    let mut snap = self.snapshot_engine.lock().await;
                    let touched = {
                        let t = self.touched_files.lock().await;
                        t.iter().cloned().collect::<Vec<_>>()
                    };
                    let _ = snap.record_turn_completed(&turn_label, &touched).await;
                }

                return Ok(final_text);
            }
        }

        // Record token usage if maximum iterations reached
        let has_known = agendex_trajectory::is_known_pricing_model(&self.config.model.model_name);
        let est_cost = agendex_trajectory::estimate_cost(
            &self.config.model.model_name,
            turn_prompt_tokens,
            turn_completion_tokens,
        );
        {
            let mut traj = self.trajectory.lock().await;
            let _ = traj.record_token_usage(
                Some(turn_step_index),
                turn_prompt_tokens,
                turn_completion_tokens,
                est_cost,
                has_known,
            );
        }

        {
            let mut snap = self.snapshot_engine.lock().await;
            let touched = {
                let t = self.touched_files.lock().await;
                t.iter().cloned().collect::<Vec<_>>()
            };
            let _ = snap.record_turn_completed(&turn_label, &touched).await;
        }

        Ok("Reached maximum tool execution iterations.".to_string())
    }

    /// Evaluates commands against an explicit safety policy before execution
    pub fn check_command_safety(command: &str) -> Result<(), String> {
        match crate::safety::CommandSafetyAssessor::evaluate(command) {
            crate::safety::CommandSafetyVerdict::Blocked { reason } => Err(format!(
                "Command '{}' denied by Agendex safety policy ({})",
                command, reason
            )),
            crate::safety::CommandSafetyVerdict::NeedsApproval { risk_reason } => Err(format!(
                "Command '{}' blocked by Agendex safety policy: requires user approval ({})",
                command, risk_reason
            )),
            crate::safety::CommandSafetyVerdict::Safe => Ok(()),
        }
    }

    async fn dispatch_tool(&self, tc: &ToolCall) -> String {
        if !self.is_tool_allowed(&tc.name) {
            return format!(
                "Tool execution error for '{}': Tool is not in allowed tools list",
                tc.name
            );
        }

        // Notify tool progress start
        let progress_msg = match tc.name.as_str() {
            "run_command" => {
                let cmd = tc
                    .arguments
                    .get("command")
                    .or_else(|| tc.arguments.get("CommandLine"))
                    .or_else(|| tc.arguments.get("cmd"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("");
                format!("[Tool: run_command] Executing '{}'", cmd)
            }
            "edit_file" => {
                let path = tc
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                format!("[Tool: edit_file] Patching '{}'", path)
            }
            "write_file" => {
                let path = tc
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                format!("[Tool: write_file] Writing '{}'", path)
            }
            "read_file" => {
                let path = tc
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                format!("[Tool: read_file] Reading '{}'", path)
            }
            other => format!("[Tool: {}] Executing...", other),
        };
        self.notify_progress(&progress_msg);

        // Safety gate on external command execution
        if tc.name == "run_command" {
            let cmd_str = tc
                .arguments
                .get("command")
                .or_else(|| tc.arguments.get("CommandLine"))
                .or_else(|| tc.arguments.get("cmd"))
                .and_then(|c| c.as_str())
                .unwrap_or("");

            match crate::safety::CommandSafetyAssessor::evaluate(cmd_str) {
                crate::safety::CommandSafetyVerdict::Blocked { reason } => {
                    tracing::warn!("Safety gate blocked command '{}': {}", cmd_str, reason);
                    return format!("Command denied by safety policy: {}", reason);
                }
                crate::safety::CommandSafetyVerdict::NeedsApproval { risk_reason } => {
                    let approved = if self
                        .auto_approve_yolo
                        .load(std::sync::atomic::Ordering::SeqCst)
                        || self.config.auto_approve_risky
                    {
                        true
                    } else if let Some(queue) = &self.approval_queue {
                        let agent_id = self.agent_id.as_deref().unwrap_or("parent");
                        queue
                            .request_approval(agent_id, cmd_str, &risk_reason)
                            .await
                    } else if let Some(handler) = &self.approval_handler {
                        handler(cmd_str, &risk_reason)
                    } else {
                        self.config.auto_approve_risky
                    };

                    if !approved {
                        tracing::warn!(
                            "Safety gate unapproved risky command '{}': {}",
                            cmd_str,
                            risk_reason
                        );
                        return format!(
                            "Command blocked: requires user approval ({})",
                            risk_reason
                        );
                    }
                }
                crate::safety::CommandSafetyVerdict::Safe => {}
            }
        }

        // Capture snapshot before mutating filesystem operations
        if tc.name == "write_file" || tc.name == "edit_file" || tc.name == "save_skill" {
            let mut snap = self.snapshot_engine.lock().await;
            let _ = snap.capture_snapshot(&format!("pre_{}", tc.name)).await;
        }

        // Route MCP tools
        if tc.name.starts_with("mcp_") {
            return match self
                .mcp_registry
                .call_tool(&tc.name, tc.arguments.clone())
                .await
            {
                Ok(res) => serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string()),
                Err(e) => format!("MCP tool execution error for '{}': {}", tc.name, e),
            };
        }

        match tc.name.as_str() {
            "send_message" => {
                if let Some(mailbox) = &self.mailbox {
                    let recipient = tc
                        .arguments
                        .get("recipient")
                        .and_then(|r| r.as_str())
                        .unwrap_or("parent");
                    let target_recipient = if recipient == "parent" {
                        self.parent_id.as_deref().unwrap_or("parent")
                    } else {
                        recipient
                    };
                    let payload = tc
                        .arguments
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("");
                    let sender = self.agent_id.as_deref().unwrap_or(&self.config.session_id);
                    let msg =
                        crate::mailbox::MailboxMessage::new(sender, target_recipient, payload);
                    match mailbox.send_message(msg).await {
                        Ok(_) => "Message sent successfully.".to_string(),
                        Err(e) => format!("Failed to send message: {}", e),
                    }
                } else {
                    "Mailbox is not configured on this agent.".to_string()
                }
            }
            "edit_file" => {
                let path = tc
                    .arguments
                    .get("path")
                    .and_then(|p| p.as_str())
                    .unwrap_or("");
                let target = tc
                    .arguments
                    .get("target")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let replacement = tc
                    .arguments
                    .get("replacement")
                    .and_then(|r| r.as_str())
                    .unwrap_or("");

                let target_path = match self.tool_registry.resolve_path(std::path::Path::new(path))
                {
                    Ok(p) => p,
                    Err(e) => return format!("Failed to edit {}: {}", path, e),
                };

                if !target_path.exists() {
                    return format!("Failed to edit {}: File does not exist", path);
                }

                let old_content = match tokio::fs::read_to_string(&target_path).await {
                    Ok(c) => c,
                    Err(e) => return format!("Failed to edit {}: {}", path, e),
                };

                let patch_res =
                    match self
                        .ast_patcher
                        .patch_string(&old_content, target, replacement)
                    {
                        Ok(res) => res,
                        Err(e) => return format!("Failed to edit {}: {}", path, e),
                    };

                let diff = agendex_ast::diff::generate_unified_diff(
                    path,
                    &old_content,
                    &patch_res.content,
                );
                if !diff.is_empty() {
                    match self.request_diff_approval(path, &diff).await {
                        DiffApprovalResult::Approved => {}
                        DiffApprovalResult::RejectedByUser => {
                            self.notify_progress(&format!(
                                "[Tool: edit_file] Edit on '{}' declined by user.",
                                path
                            ));
                            return "Edit declined by user".to_string();
                        }
                        DiffApprovalResult::RejectedNonTerminal => {
                            self.notify_progress(&format!(
                                "[Tool: edit_file] Edit on '{}' rejected: requires interactive terminal approval or -y/--auto-approve flag.",
                                path
                            ));
                            return "Edit rejected: non-interactive session requires -y/--auto-approve to apply changes without terminal prompt.".to_string();
                        }
                    }
                }

                let result = match self
                    .ast_patcher
                    .patch_file(&target_path, target, replacement)
                {
                    Ok(res) => {
                        let norm = agendex_trajectory::normalize_rel_path(
                            path,
                            &self.config.workspace_dir,
                        );
                        self.touched_files.lock().await.insert(norm);
                        format!(
                            "Successfully replaced {} occurrences in {}",
                            res.occurrences, path
                        )
                    }
                    Err(e) => format!("Failed to edit {}: {}", path, e),
                };
                self.notify_progress(&format!("[Tool: edit_file] Completed: {}", path));
                result
            }
            "save_skill" => {
                let name = tc
                    .arguments
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let description = tc
                    .arguments
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = tc
                    .arguments
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if name.trim().is_empty() || content.trim().is_empty() {
                    return "Failed to save skill: 'name' and 'content' parameters are required."
                        .to_string();
                }

                let safe_name = match agendex_scaffold::sanitize_skill_name(name) {
                    Ok(n) => n,
                    Err(e) => return format!("Failed to save skill: {}", e),
                };

                let saver = agendex_scaffold::SkillSaver::new(&self.config.workspace_dir);
                let target_path = match saver.resolve_skill_path(&safe_name) {
                    Ok(p) => p,
                    Err(e) => return format!("Failed to save skill: {}", e),
                };
                let already_exists = target_path.exists();

                let action = if already_exists {
                    format!("save_skill (OVERWRITE): {}", safe_name)
                } else {
                    format!("save_skill: {}", safe_name)
                };

                let content_preview = if content.len() > 400 {
                    let safe_idx = content.floor_char_boundary(400);
                    format!("{}...", content[..safe_idx].trim())
                } else {
                    content.trim().to_string()
                };

                let reason = format!(
                    "{}\nPath: {}\nDescription: {}\nProposed Content Preview:\n---\n{}\n---",
                    if already_exists {
                        "WARNING: Overwriting existing skill file! Check instructions carefully to prevent prompt injection."
                    } else {
                        "Writing persistent skill playbook"
                    },
                    target_path.display(),
                    description,
                    content_preview
                );

                let approved = if let Some(queue) = &self.approval_queue {
                    let agent_id = self.agent_id.as_deref().unwrap_or("parent");
                    queue.request_approval(agent_id, &action, &reason).await
                } else if let Some(handler) = &self.approval_handler {
                    handler(&action, &reason)
                } else if already_exists {
                    // Overwriting existing skill playbook strictly requires explicit user approval
                    false
                } else {
                    self.config.auto_approve_risky
                };

                if !approved {
                    tracing::warn!(
                        "Safety gate rejected save_skill for '{}': {}",
                        safe_name,
                        reason
                    );
                    if already_exists {
                        return format!(
                            "Skill creation for '{}' blocked: overwriting an existing skill requires explicit user approval",
                            safe_name
                        );
                    }
                    return format!(
                        "Skill creation for '{}' blocked: requires user approval",
                        safe_name
                    );
                }

                let args = agendex_scaffold::SaveSkillArgs {
                    name: safe_name.clone(),
                    description: description.to_string(),
                    content: content.to_string(),
                    overwrite: already_exists,
                };

                match saver.save_skill_direct(&args) {
                    Ok(res) => {
                        // Immediately register the newly saved skill into the registry
                        if let Ok(skill) =
                            agendex_scaffold::SkillRegistry::parse_skill_file(&res.path)
                        {
                            if let Ok(mut reg) = self.skill_registry.write() {
                                reg.register(skill);
                            }
                        }

                        format!(
                            "Successfully saved skill '{}' to {} ({} bytes written).",
                            res.name,
                            res.path.display(),
                            res.bytes_written
                        )
                    }
                    Err(e) => format!("Failed to save skill '{}': {}", safe_name, e),
                }
            }
            _ => {
                if tc.name == "write_file" {
                    let path_str = tc
                        .arguments
                        .get("path")
                        .and_then(|p| p.as_str())
                        .unwrap_or("");
                    let content = tc
                        .arguments
                        .get("content")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    let overwrite = tc
                        .arguments
                        .get("overwrite")
                        .and_then(|o| o.as_bool())
                        .unwrap_or(false);
                    let append = tc
                        .arguments
                        .get("append")
                        .and_then(|a| a.as_bool())
                        .unwrap_or(false);

                    if let Ok(target_path) = self
                        .tool_registry
                        .resolve_path(std::path::Path::new(path_str))
                    {
                        if target_path.exists() && (overwrite || append) {
                            let old_content = tokio::fs::read_to_string(&target_path)
                                .await
                                .unwrap_or_default();
                            let new_content = if append {
                                format!("{}{}", old_content, content)
                            } else {
                                content.to_string()
                            };

                            if old_content != new_content {
                                let diff = agendex_ast::diff::generate_unified_diff(
                                    path_str,
                                    &old_content,
                                    &new_content,
                                );
                                if !diff.is_empty() {
                                    match self.request_diff_approval(path_str, &diff).await {
                                        DiffApprovalResult::Approved => {}
                                        DiffApprovalResult::RejectedByUser => {
                                            self.notify_progress(&format!(
                                                "[Tool: write_file] Edit on '{}' declined by user.",
                                                path_str
                                            ));
                                            return "Edit declined by user".to_string();
                                        }
                                        DiffApprovalResult::RejectedNonTerminal => {
                                            self.notify_progress(&format!(
                                                "[Tool: write_file] Edit on '{}' rejected: requires interactive terminal approval or -y/--auto-approve flag.",
                                                path_str
                                            ));
                                            return "Edit rejected: non-interactive session requires -y/--auto-approve to apply changes without terminal prompt.".to_string();
                                        }
                                    }
                                }
                            }
                        } else if !target_path.exists() {
                            // Review diff for proposed new file creation
                            let diff =
                                agendex_ast::diff::generate_unified_diff(path_str, "", content);
                            if !diff.is_empty() {
                                match self.request_diff_approval(path_str, &diff).await {
                                    DiffApprovalResult::Approved => {}
                                    DiffApprovalResult::RejectedByUser => {
                                        self.notify_progress(&format!(
                                            "[Tool: write_file] File creation of '{}' declined by user.",
                                            path_str
                                        ));
                                        return "File creation declined by user".to_string();
                                    }
                                    DiffApprovalResult::RejectedNonTerminal => {
                                        self.notify_progress(&format!(
                                            "[Tool: write_file] File creation of '{}' rejected: requires interactive terminal approval or -y/--auto-approve flag.",
                                            path_str
                                        ));
                                        return "File creation rejected: non-interactive session requires -y/--auto-approve to apply changes without terminal prompt.".to_string();
                                    }
                                }
                            }
                        }
                    }
                }

                let pre_cmd_state = if tc.name == "run_command" {
                    let out_untracked = tokio::process::Command::new("git")
                        .args(["ls-files", "--others", "--exclude-standard"])
                        .current_dir(&self.config.workspace_dir)
                        .output()
                        .await;
                    let untracked = out_untracked
                        .ok()
                        .map(|o| {
                            String::from_utf8_lossy(&o.stdout)
                                .lines()
                                .map(|l| l.trim().replace('\\', "/"))
                                .filter(|l| !l.is_empty())
                                .collect::<Vec<String>>()
                        })
                        .unwrap_or_default();

                    let out_modified = tokio::process::Command::new("git")
                        .args(["diff", "--name-only"])
                        .current_dir(&self.config.workspace_dir)
                        .output()
                        .await;
                    let modified = out_modified
                        .ok()
                        .map(|o| {
                            String::from_utf8_lossy(&o.stdout)
                                .lines()
                                .map(|l| l.trim().replace('\\', "/"))
                                .filter(|l| !l.is_empty())
                                .collect::<Vec<String>>()
                        })
                        .unwrap_or_default();

                    Some((untracked, modified))
                } else {
                    None
                };

                let res_val = self
                    .tool_registry
                    .execute_tool(&tc.name, tc.arguments.clone())
                    .await;

                if let Some((pre_untracked, pre_modified)) = pre_cmd_state {
                    let post_untracked_out = tokio::process::Command::new("git")
                        .args(["ls-files", "--others", "--exclude-standard"])
                        .current_dir(&self.config.workspace_dir)
                        .output()
                        .await;
                    if let Ok(o) = post_untracked_out {
                        let mut snap = self.snapshot_engine.lock().await;
                        let mut touched = self.touched_files.lock().await;
                        for line in String::from_utf8_lossy(&o.stdout).lines() {
                            let trimmed = line.trim().replace('\\', "/");
                            if !trimmed.is_empty() && !pre_untracked.contains(&trimmed) {
                                snap.record_created_file(&trimmed);
                                touched.insert(trimmed);
                            }
                        }
                    }

                    let post_diff_out = tokio::process::Command::new("git")
                        .args(["diff", "--name-only"])
                        .current_dir(&self.config.workspace_dir)
                        .output()
                        .await;
                    if let Ok(o) = post_diff_out {
                        let mut touched = self.touched_files.lock().await;
                        for line in String::from_utf8_lossy(&o.stdout).lines() {
                            let trimmed = line.trim().replace('\\', "/");
                            if !trimmed.is_empty() && !pre_modified.contains(&trimmed) {
                                touched.insert(trimmed);
                            }
                        }
                    }
                }

                match res_val {
                    Ok(res) => {
                        if tc.name == "write_file" {
                            if let Some(path_str) = res.get("path").and_then(|p| p.as_str()) {
                                let norm = agendex_trajectory::normalize_rel_path(
                                    path_str,
                                    &self.config.workspace_dir,
                                );
                                self.touched_files.lock().await.insert(norm.clone());
                                if res
                                    .get("created")
                                    .and_then(|c| c.as_bool())
                                    .unwrap_or(false)
                                {
                                    let mut snap = self.snapshot_engine.lock().await;
                                    snap.record_created_file(&norm);
                                }
                            }
                        }
                        self.notify_progress(&format!("[Tool: {}] Completed.", tc.name));
                        let out_str =
                            serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string());
                        if tc.name == "read_file" || tc.name == "run_command" {
                            return agendex_trajectory::mask_secrets(&out_str);
                        }
                        out_str
                    }
                    Err(e) => {
                        self.notify_progress(&format!("[Tool: {}] Error: {}", tc.name, e));
                        format!("Tool execution error for '{}': {}", tc.name, e)
                    }
                }
            }
        }
    }

    /// Returns the token usage breakdown from the most recent turn.
    pub async fn last_turn_usage(&self) -> Option<agendex_trajectory::TokenUsage> {
        let traj = self.trajectory.lock().await;
        traj.last_turn_usage()
    }

    /// Returns the cumulative session token usage and estimated cost.
    pub async fn session_usage(&self) -> agendex_trajectory::SessionUsage {
        let traj = self.trajectory.lock().await;
        traj.session_usage()
    }

    /// Formats the most recent turn and cumulative session usage into standard telemetry string:
    /// e.g. "[Tokens: 412 in, 85 out | Session: 1,230 ($0.0018)]"
    pub async fn format_telemetry(&self) -> Option<String> {
        let traj = self.trajectory.lock().await;
        traj.format_last_telemetry()
    }

    /// Handles `/provider` or `/providers` slash command input.
    pub async fn handle_provider_command(&self, input: &str) -> anyhow::Result<String> {
        let parts: Vec<&str> = input.split_whitespace().collect();
        if parts.len() <= 1 {
            return Ok(self.list_providers().await);
        }

        match parts[1].to_lowercase().as_str() {
            "list" | "ls" => Ok(self.list_providers().await),
            "switch" | "use" => {
                if parts.len() < 3 {
                    Ok("Usage: /provider switch <name> [model]\nExample: /provider switch strata\n         /provider switch nim meta/llama-3.1-70b-instruct".to_string())
                } else {
                    let provider = parts[2];
                    let model = parts.get(3).copied();
                    self.switch_provider(provider, model).await
                }
            }
            "model" | "set-model" => {
                if parts.len() < 3 {
                    Ok("Usage: /provider model <model_name>\nExample: /provider model mistralai/mistral-large-2-instruct".to_string())
                } else {
                    self.set_provider_model(parts[2]).await
                }
            }
            "add" => {
                if let Some(parsed) = crate::config::parse_provider_add_tokens(&parts) {
                    self.add_provider(
                        &parsed.name,
                        &parsed.base_url,
                        parsed.model.as_deref(),
                        parsed.format.as_deref(),
                        parsed.api_key.as_deref(),
                    )
                    .await
                } else {
                    Ok("Usage: /provider add <name> [base_url] [model] [format] [api_key]\nExample: /provider add vllm http://localhost:8000/v1 meta-llama/Llama-3-8B openai $VLLM_KEY\n         /provider add nim nvapi-secret meta/llama-3.1-70b-instruct".to_string())
                }
            }
            "help" | "-h" | "--help" => Ok(self.provider_help()),
            other => {
                // Shortcut: e.g. `/provider strata` or `/provider nim`
                let model = parts.get(2).copied();
                self.switch_provider(other, model).await
            }
        }
    }

    fn provider_help(&self) -> String {
        r#"Provider Management Commands:
  /provider                        - List all configured providers and status
  /provider list                   - List providers
  /provider switch <name> [model]  - Switch active provider (e.g. /provider switch strata)
  /provider <name> [model]         - Quick-switch provider (e.g. /provider nim)
  /provider model <model_name>     - Change active model for current provider
  /provider add <name> <url> [m] [fmt] [key] - Add/update custom provider"#
            .to_string()
    }

    /// Returns a formatted list of all configured providers and indicates the active one.
    pub async fn list_providers(&self) -> String {
        let cfg = crate::config::load_default_config()
            .unwrap_or_else(crate::config::create_default_config_with_presets);
        let active = self.active_provider_name.read().await.clone();
        let cur_model = self.active_model_config.read().await.clone();

        let mut out = String::new();
        out.push_str(&format!(
            "\n  {}╭──────────────────────────────────────────────────────────────────────────╮{}\n",
            crate::ui::TEXT_MUTED, crate::ui::RESET
        ));
        out.push_str(&format!(
            "  {}│{}  {}✦ Active LLM Provider:{} {}{:<47}{} {}│{}\n",
            crate::ui::TEXT_MUTED,
            crate::ui::RESET,
            crate::ui::ACCENT_CYAN,
            crate::ui::RESET,
            crate::ui::TEXT_PRIMARY,
            active,
            crate::ui::RESET,
            crate::ui::TEXT_MUTED,
            crate::ui::RESET
        ));
        out.push_str(&format!(
            "  {}│{}    Model:    {}{:<50}{} {}│{}\n",
            crate::ui::TEXT_MUTED,
            crate::ui::RESET,
            crate::ui::TEXT_PRIMARY,
            cur_model.model_name,
            crate::ui::RESET,
            crate::ui::TEXT_MUTED,
            crate::ui::RESET
        ));
        out.push_str(&format!(
            "  {}│{}    Endpoint: {}{:<50}{} {}│{}\n",
            crate::ui::TEXT_MUTED,
            crate::ui::RESET,
            crate::ui::TEXT_SECONDARY,
            cur_model
                .base_url
                .as_deref()
                .unwrap_or("<upstream default>"),
            crate::ui::RESET,
            crate::ui::TEXT_MUTED,
            crate::ui::RESET
        ));
        out.push_str(&format!(
            "  {}╰──────────────────────────────────────────────────────────────────────────╯{}\n\n",
            crate::ui::TEXT_MUTED, crate::ui::RESET
        ));

        out.push_str(&format!(
            "  {}Available Providers:{} \n",
            crate::ui::TEXT_PRIMARY,
            crate::ui::RESET
        ));

        let mut keys: Vec<&String> = cfg.providers.keys().collect();
        keys.sort();

        for key in keys {
            let entry = &cfg.providers[key];
            let is_active = key.eq_ignore_ascii_case(&active);
            let marker = if is_active {
                format!("{}*{}", crate::ui::ACCENT_GREEN, crate::ui::RESET)
            } else {
                " ".to_string()
            };
            let model = entry
                .model
                .as_deref()
                .unwrap_or_else(|| crate::config::default_model_for_preset(key));
            let url = entry.base_url.as_deref().unwrap_or("<upstream default>");
            out.push_str(&format!(
                "   {} {:<12} {}{:<32}{} {}{}{}\n",
                marker,
                key,
                crate::ui::TEXT_PRIMARY,
                model,
                crate::ui::RESET,
                crate::ui::TEXT_MUTED,
                url,
                crate::ui::RESET
            ));
        }

        out.push_str(&format!(
            "\n  {}Commands:{} {}/provider switch <name>{} · {}/provider model <name>{} · {}/provider add <name> <url>{}\n",
            crate::ui::TEXT_MUTED,
            crate::ui::RESET,
            crate::ui::ACCENT_CYAN,
            crate::ui::RESET,
            crate::ui::ACCENT_CYAN,
            crate::ui::RESET,
            crate::ui::ACCENT_CYAN,
            crate::ui::RESET,
        ));
        out
    }

    /// Switches the active LLM provider and re-initializes the active model client in-flight.
    pub async fn switch_provider(
        &self,
        provider_name: &str,
        model_override: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut cfg = crate::config::load_default_config()
            .unwrap_or_else(crate::config::create_default_config_with_presets);

        let provider_key = provider_name.to_lowercase();
        let (format, base_url, model_name, api_key, tier) =
            if let Some(entry) = cfg.providers.get(&provider_key).cloned() {
                let fmt = entry
                    .format
                    .as_deref()
                    .map(|f| crate::config::resolve_provider_preset(f).0)
                    .unwrap_or_else(|| crate::config::resolve_provider_preset(&provider_key).0);
                let url = entry
                    .base_url
                    .clone()
                    .or_else(|| crate::config::resolve_provider_preset(&provider_key).1);
                let mdl = model_override
                    .map(|s| s.to_string())
                    .or(entry.model.clone())
                    .unwrap_or_else(|| {
                        crate::config::default_model_for_preset(&provider_key).to_string()
                    });
                let key = entry
                    .api_key
                    .as_deref()
                    .map(crate::config::expand_env_var_or_literal)
                    .unwrap_or_else(|| crate::config::default_api_key_for_preset(&provider_key));
                let tier = entry
                    .tier
                    .as_deref()
                    .map(crate::config::parse_tier_string)
                    .unwrap_or(ModelTier::Standard128k);
                (fmt, url, mdl, key, tier)
            } else {
                let (fmt, default_base) = crate::config::resolve_provider_preset(&provider_key);
                let mdl = model_override.map(|s| s.to_string()).unwrap_or_else(|| {
                    crate::config::default_model_for_preset(&provider_key).to_string()
                });
                let key = crate::config::default_api_key_for_preset(&provider_key);
                (fmt, default_base, mdl, key, ModelTier::Standard128k)
            };

        let new_model_config = ModelConfig {
            format,
            base_url: base_url.clone(),
            api_key: api_key.clone(),
            model_name: model_name.clone(),
            temperature: Some(0.2),
            max_output_tokens: Some(4096),
        };

        // Update in-memory state
        *self.active_provider_name.write().await = provider_key.clone();
        *self.active_model_config.write().await = new_model_config.clone();
        *self.model_client.write().await = ModelClient::new(new_model_config);

        // Update persistent config on disk
        cfg.active_provider = Some(provider_key.clone());
        cfg.model = Some(model_name.clone());
        cfg.base_url = base_url.clone();
        cfg.provider = Some(provider_key.clone());
        if let Some(ref m) = model_override {
            if let Some(p) = cfg.providers.get_mut(&provider_key) {
                p.model = Some(m.to_string());
            }
        }
        if let Some(cfg_path) = crate::config::default_config_path() {
            let _ = cfg.save_to_disk(&cfg_path);
        }

        let out = format!(
            "[Agendex] Switched active provider to '{}'.\n  Model:    {}\n  Format:   {:?}\n  Base URL: {}\n  Tier:     {:?}",
            provider_key,
            model_name,
            format,
            base_url.as_deref().unwrap_or("<upstream default>"),
            tier
        );
        Ok(out)
    }

    /// Sets the model name for the current active provider.
    pub async fn set_provider_model(&self, model_name: &str) -> anyhow::Result<String> {
        let active = self.active_provider_name.read().await.clone();
        let mut model_cfg = self.active_model_config.read().await.clone();
        model_cfg.model_name = model_name.to_string();

        *self.active_model_config.write().await = model_cfg.clone();
        *self.model_client.write().await = ModelClient::new(model_cfg);

        let mut cfg = crate::config::load_default_config()
            .unwrap_or_else(crate::config::create_default_config_with_presets);
        if let Some(entry) = cfg.providers.get_mut(&active) {
            entry.model = Some(model_name.to_string());
        }
        cfg.model = Some(model_name.to_string());
        if let Some(cfg_path) = crate::config::default_config_path() {
            let _ = cfg.save_to_disk(&cfg_path);
        }

        Ok(format!(
            "[Agendex] Updated model for provider '{}' to '{}'.",
            active, model_name
        ))
    }

    /// Adds or updates a provider entry in the persistent configuration.
    pub async fn add_provider(
        &self,
        name: &str,
        base_url: &str,
        model: Option<&str>,
        format: Option<&str>,
        api_key: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut cfg = crate::config::load_default_config()
            .unwrap_or_else(crate::config::create_default_config_with_presets);

        let key = name.to_lowercase();
        let default_m = crate::config::default_model_for_preset(&key);
        let resolved_format = format.unwrap_or("openai").to_string();
        let resolved_model = model.unwrap_or(default_m).to_string();
        let resolved_key = api_key.unwrap_or("").to_string();

        let (stored_key_ref, key_display_note) = if resolved_key.is_empty() {
            (None, String::new())
        } else if resolved_key.starts_with('$') {
            (
                Some(resolved_key.clone()),
                format!(" (key ref: {})", resolved_key),
            )
        } else {
            let env_var = format!("{}_API_KEY", key.to_uppercase().replace('-', "_"));
            let _ = crate::config::persist_agendex_env_var(&env_var, &resolved_key);
            std::env::set_var(&env_var, &resolved_key);
            let env_ref = format!("${}", env_var);
            (
                Some(env_ref.clone()),
                format!(" (key persisted to .env as {})", env_ref),
            )
        };

        cfg.providers.insert(
            key.clone(),
            crate::config::ProviderConfigEntry {
                format: Some(resolved_format.clone()),
                base_url: Some(base_url.to_string()),
                model: Some(resolved_model.clone()),
                api_key: stored_key_ref,
                tier: Some("128k".to_string()),
            },
        );

        if let Some(cfg_path) = crate::config::default_config_path() {
            cfg.save_to_disk(&cfg_path)?;
        }

        Ok(format!(
            "[Agendex] Added provider '{}' (url: {}, model: {}, format: {}){}.\nTo activate: '/provider switch {}'",
            key, base_url, resolved_model, resolved_format, key_display_note, key
        ))
    }

    /// Handles `/help` slash command, listing all core commands, tools, and dynamic skills.
    pub fn handle_help_command(&self) -> String {
        let mut out = String::new();
        out.push_str("✦ Agendex Command & Skill Switchboard ✦\n\n");
        out.push_str("Core Interactive Commands:\n");
        out.push_str("  /model, /provider    Open interactive model and provider selector\n");
        out.push_str("  /diff                Show git diff of all agent changes this session\n");
        out.push_str("  /undo, /revert       Undo the last turn and restore snapshot\n");
        out.push_str("  /yolo                Toggle auto-approve on/off dynamically\n");
        out.push_str("  /cost                Show token cost breakdown in USD\n");
        out.push_str("  /tokens              Show prompt, completion, and cached token metrics\n");
        out.push_str("  /context             Display visual context window capacity gauge\n");
        out.push_str("  /compact             Manually trigger context compaction\n");
        out.push_str("  /clear               Clear the terminal screen\n");
        out.push_str("  /new                 Start a fresh conversation session\n");
        out.push_str("  /resume [id]         List or resume past session trajectories\n");
        out.push_str("  /config              Display current configuration file parameters\n");
        out.push_str("  /def <symbol>        Jump to AST symbol definition in codebase\n");
        out.push_str("  /exit, /quit         Exit interactive session\n\n");

        out.push_str("Scaffolding & Capabilities:\n");
        out.push_str("  /skills              List all discovered skills and their source paths\n");
        out.push_str("  /mcp                 List registered MCP servers and tool counts\n");
        out.push_str("  /tools               List all available native harness and MCP tools\n");
        out.push_str("  /init                Scan project layout and bootstrap AGENTS.md\n\n");

        if let Ok(reg) = self.skill_registry.read() {
            let skills = reg.list_skills();
            if !skills.is_empty() {
                out.push_str("Discovered Skills:\n");
                for s in skills {
                    out.push_str(&format!("  {:<20} {}\n", s.command, s.description));
                }
            }
        }
        out
    }

    /// Handles `/cost` command.
    pub async fn handle_cost_command(&self) -> String {
        let usage = self.session_usage().await;
        let last = self.last_turn_usage().await;
        let mut out = String::new();
        out.push_str("Session Cost Breakdown:\n");
        if let Some(l) = last {
            let last_cost_str = if l.has_known_cost {
                format!("${:.4}", l.estimated_cost)
            } else {
                "n/a".to_string()
            };
            out.push_str(&format!(
                "  • Last Turn Cost:    {} ({} in, {} out)\n",
                last_cost_str, l.prompt_tokens, l.completion_tokens
            ));
        }
        let total_cost_str = if usage.has_known_cost {
            format!("${:.4}", usage.total_cost)
        } else {
            "n/a".to_string()
        };
        out.push_str(&format!(
            "  • Cumulative Cost:   {} across {} turn(s)\n",
            total_cost_str, usage.turn_count
        ));
        out.push_str(&format!(
            "  • Total Volume:      {} prompt + {} completion = {} tokens\n",
            format_num(usage.total_prompt_tokens),
            format_num(usage.total_completion_tokens),
            format_num(usage.total_tokens)
        ));
        out
    }

    /// Handles `/tokens` command.
    pub async fn handle_tokens_command(&self) -> String {
        let usage = self.session_usage().await;
        let last = self.last_turn_usage().await;
        let mut out = String::new();
        out.push_str("Session Token Telemetry:\n");
        if let Some(l) = last {
            out.push_str(&format!(
                "  • Last Turn Input:      {}\n",
                format_num(l.prompt_tokens)
            ));
            out.push_str(&format!(
                "  • Last Turn Output:     {}\n",
                format_num(l.completion_tokens)
            ));
            out.push_str(&format!(
                "  • Last Turn Total:      {}\n",
                format_num(l.total_tokens)
            ));
        }
        out.push_str(&format!(
            "  • Cumulative Input:     {}\n",
            format_num(usage.total_prompt_tokens)
        ));
        out.push_str(&format!(
            "  • Cumulative Output:    {}\n",
            format_num(usage.total_completion_tokens)
        ));
        out.push_str(&format!(
            "  • Cumulative Total:     {}\n",
            format_num(usage.total_tokens)
        ));
        out.push_str(&format!("  • Completed Turns:      {}\n", usage.turn_count));
        out
    }

    /// Handles `/context` command.
    pub async fn handle_context_command(&self) -> String {
        let last_known_prompt_tokens = self
            .current_turn_prompt_tokens
            .load(std::sync::atomic::Ordering::SeqCst);

        let active_tokens = if last_known_prompt_tokens > 0 {
            last_known_prompt_tokens
        } else {
            // Precise pre-turn estimation including full system prompt and wire tool schemas
            let slash_prompt = self
                .skill_registry
                .read()
                .map(|r| r.format_slash_commands_prompt())
                .unwrap_or_default();
            let system_prompt = format!(
                "You are Agendex, a powerful local-first autonomous agent.\n\
                Workspace: {}\n\
                Available Slash Commands:\n{}\n\
                Active Rules:\n{}",
                self.config.workspace_dir.display(),
                slash_prompt,
                self.rules_engine.generate_rule_blocks(None)
            );

            let mut prompt_chars = system_prompt.len();

            let history = self.history.lock().await;
            for m in history.iter() {
                prompt_chars += m.content.len();
                if let Some(tcs) = &m.tool_calls {
                    if let Ok(s) = serde_json::to_string(tcs) {
                        prompt_chars += s.len();
                    }
                }
            }
            drop(history);

            // Tools formatted as wire function definitions in JSON (~2.8 chars per token)
            let mut tool_chars = 0usize;
            for t in self.tool_registry.list_tool_definitions() {
                if let Ok(s) = serde_json::to_string(&serde_json::json!({
                    "type": "function",
                    "function": t
                })) {
                    tool_chars += s.len();
                }
            }
            for t in self.mcp_registry.list_tools().await {
                if let Ok(s) = serde_json::to_string(&serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description.unwrap_or_default(),
                        "parameters": t.input_schema,
                    }
                })) {
                    tool_chars += s.len();
                }
            }

            // Text tokens (~3.6 chars/token) + JSON schema tokens (~2.8 chars/token)
            let text_tokens = prompt_chars / 4;
            let schema_tokens = (tool_chars as f64 / 2.8).round() as usize;
            (text_tokens + schema_tokens).max(1)
        };

        let tier = self.config.model_tier;
        let limit = tier.ceiling_tokens();
        let clamped_tokens = active_tokens.min(limit);
        let ratio = (clamped_tokens as f64 / limit as f64).clamp(0.0, 1.0);
        let bar_len: usize = 24;
        let filled = (ratio * bar_len as f64).round() as usize;
        let empty = bar_len.saturating_sub(filled);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
        let pct = ratio * 100.0;
        let model_name = self.active_model_config.read().await.model_name.clone();

        format!(
            "Context Window Gauge [{}]:\n  [{}] {:>5.1}% ({} / {} tokens)\n  Compaction Tier: {:?} (triggers at ceiling)",
            model_name,
            bar,
            pct,
            format_num(clamped_tokens),
            format_num(limit),
            tier
        )
    }

    /// Handles `/diff` command.
    pub async fn handle_diff_command(&self) -> anyhow::Result<String> {
        let snap = self.snapshot_engine.lock().await;
        let snapshots = snap.list_snapshots();
        if let Some(first) = snapshots.first() {
            let diff = snap.diff_snapshot(&first.tree_hash).await?;
            if diff.trim().is_empty() {
                Ok("No workspace modifications made in this session (working tree matches baseline).".to_string())
            } else {
                Ok(diff)
            }
        } else {
            Ok("No baseline snapshot available to diff against.".to_string())
        }
    }

    /// Handles `/skills` command.
    pub fn handle_skills_command(&self) -> String {
        let mut out = String::new();
        if let Ok(reg) = self.skill_registry.read() {
            let skills = reg.list_skills();
            if skills.is_empty() {
                return "No skills currently discovered in .agents/skills/ or .agenda/skills/."
                    .to_string();
            }
            out.push_str(&format!("Discovered Skills ({} total):\n\n", skills.len()));
            for s in skills {
                out.push_str(&format!(
                    "• {:<16} ({})\n  Description: {}\n  Source:      {}\n\n",
                    s.name,
                    s.command,
                    s.description,
                    s.path.display()
                ));
            }
        }
        out
    }

    /// Handles `/mcp` command.
    pub async fn handle_mcp_command(&self) -> String {
        let tools = self.mcp_registry.list_tools().await;
        if tools.is_empty() {
            return "No MCP servers connected or declared in .agendex/mcp.json.".to_string();
        }
        let mut out = format!("Connected MCP Tools ({} total):\n\n", tools.len());
        for t in &tools {
            out.push_str(&format!(
                "• [{}] {}\n  Description: {}\n",
                t.server_name.as_deref().unwrap_or("mcp"),
                t.name,
                t.description.as_deref().unwrap_or("No description")
            ));
        }
        out
    }

    /// Handles `/tools` command.
    pub async fn handle_tools_command(&self) -> String {
        let mut out = String::new();
        out.push_str("Registered Agent Execution Tools:\n\n");
        out.push_str("Native Harness Tools:\n");
        out.push_str("  • read_file         Read file contents with optional line boundaries\n");
        out.push_str("  • write_file        Create or overwrite files in the workspace\n");
        out.push_str(
            "  • edit_file         Exact AST/text replacement with syntax gate validation\n",
        );
        out.push_str("  • run_command       Execute shell commands with safety policy gate\n");
        out.push_str(
            "  • find_definition   Find AST symbol definition (functions, classes, types)\n",
        );
        out.push_str(
            "  • search_history    Search past conversation trajectories across sessions\n",
        );
        out.push_str(
            "  • save_skill        Persist multi-turn procedural instructions to SKILL.md\n",
        );
        out.push_str("  • synthesize_tool   Synthesize and sandbox ad-hoc executable tools\n");

        let mcp_tools = self.mcp_registry.list_tools().await;
        if !mcp_tools.is_empty() {
            out.push_str(&format!("\nMCP Tools ({}):\n", mcp_tools.len()));
            for t in &mcp_tools {
                out.push_str(&format!(
                    "  • [{}] {:<20} {}\n",
                    t.server_name.as_deref().unwrap_or("mcp"),
                    t.name,
                    t.description.as_deref().unwrap_or("No description")
                ));
            }
        }
        out
    }

    /// Handles `/init` command.
    pub async fn handle_init_command(&self) -> anyhow::Result<String> {
        let agents_md_path = self.config.workspace_dir.join("AGENTS.md");
        if agents_md_path.exists() {
            return Ok(format!(
                "AGENTS.md already exists at {}. Scaffolding verified.",
                agents_md_path.display()
            ));
        }

        let mut tech_stack = Vec::new();
        if self.config.workspace_dir.join("Cargo.toml").exists() {
            tech_stack.push("Rust (Cargo)");
        }
        if self.config.workspace_dir.join("package.json").exists() {
            tech_stack.push("JavaScript / TypeScript (Node)");
        }
        if self.config.workspace_dir.join("pyproject.toml").exists()
            || self.config.workspace_dir.join("requirements.txt").exists()
        {
            tech_stack.push("Python");
        }
        if self.config.workspace_dir.join("go.mod").exists() {
            tech_stack.push("Go");
        }
        let stack_desc = if tech_stack.is_empty() {
            "General Codebase".to_string()
        } else {
            tech_stack.join(", ")
        };

        let project_name = self
            .config
            .workspace_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Project");

        let content = format!(
            "# Agent Guidelines & Project Routing Hub (`AGENTS.md`)\n\n\
            > **Tier 1 Context Router** — Operational directives and architecture switchboard for autonomous agents.\n\n\
            ---\n\n\
            ## 1. Project Mission & Technology Stack\n\n\
            **{project_name}** is developed in {stack_desc}.\n\
            - **Workspace Root**: `{workspace}`\n\
            - **Verification Law**: Every modification must compile and pass existing test suites cleanly before committing.\n\
            - **Snapshot Integrity**: Mutating edits must be verified against snapshots or git working tree.\n\n\
            ---\n\n\
            ## 2. Operational Rules & DoD\n\n\
            1. **Verify Before Declaring Done**: Execute tests and build commands directly; never report work done without affirmative terminal proof.\n\
            2. **Deterministic Tool Results**: Handle tool errors with structured output; avoid unverified assumptions.\n\
            3. **Clean Code & Commits**: Keep edits modular and bounded; write code >10 lines to concrete files.\n\n\
            ---\n\n\
            ## 3. Directory Layout & Key Entrypoints\n\n\
            Review workspace files and add key architecture entrypoints here.\n",
            project_name = project_name,
            stack_desc = stack_desc,
            workspace = self.config.workspace_dir.display()
        );

        tokio::fs::write(&agents_md_path, content).await?;
        let _ = tokio::fs::create_dir_all(self.config.workspace_dir.join(".agents").join("skills"))
            .await;
        let _ = tokio::fs::create_dir_all(self.config.workspace_dir.join(".agents").join("rules"))
            .await;

        Ok(format!(
            "Successfully initialized AGENTS.md and .agents/ scaffolding at {}.",
            self.config.workspace_dir.display()
        ))
    }

    /// Handles `/config` command.
    pub fn handle_config_command(&self) -> String {
        let home_dir = std::env::var("AGENDEX_HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        let config_path = std::path::PathBuf::from(home_dir)
            .join(".agendex")
            .join("config.toml");
        let active_prov = self
            .active_provider_name
            .try_read()
            .map(|p| p.clone())
            .unwrap_or_else(|_| "unknown".to_string());
        let active_mdl = self
            .active_model_config
            .try_read()
            .map(|m| m.model_name.clone())
            .unwrap_or_else(|_| self.config.model.model_name.clone());
        let yolo = self
            .auto_approve_yolo
            .load(std::sync::atomic::Ordering::SeqCst);

        format!(
            "Agendex Configuration:\n  Config Path:        {}\n  Active Provider:    {}\n  Active Model:       {}\n  Model Tier Ceiling: {:?}\n  Auto-Approve (YOLO):{}\n  Workspace:          {}",
            config_path.display(),
            active_prov,
            active_mdl,
            self.config.model_tier,
            if yolo { " ENABLED" } else { " DISABLED" },
            self.config.workspace_dir.display()
        )
    }

    /// Handles `/new` command.
    pub async fn handle_new_command(&self) -> anyhow::Result<String> {
        let new_session_id = uuid::Uuid::new_v4().to_string();
        let snap = self.snapshot_engine.lock().await;
        let new_log_dir = snap.state_dir().join("logs").join(&new_session_id);
        drop(snap);

        tokio::fs::create_dir_all(&new_log_dir).await?;
        let mut traj = self.trajectory.lock().await;
        *traj = agendex_trajectory::TrajectoryLogger::new(&new_log_dir)?;

        self.touched_files.lock().await.clear();

        Ok(format!(
            "Started fresh session: {}\nTrajectory logs streaming to: {}",
            new_session_id,
            new_log_dir.display()
        ))
    }

    /// Handles `/resume` command.
    pub async fn handle_resume_command(&self, args: &str) -> anyhow::Result<String> {
        let trimmed_arg = args.trim();
        let snap = self.snapshot_engine.lock().await;
        let logs_dir = snap.state_dir().join("logs");
        drop(snap);

        if !trimmed_arg.is_empty() {
            if trimmed_arg.contains("..") || trimmed_arg.contains('/') || trimmed_arg.contains('\\')
            {
                return Ok(format!(
                    "Invalid session ID '{}': directory traversal characters are prohibited.",
                    trimmed_arg
                ));
            }
            let target_log_dir = logs_dir.join(trimmed_arg);
            if !target_log_dir.exists() {
                return Ok(format!(
                    "Session '{}' not found in logs directory.",
                    trimmed_arg
                ));
            }
            if let (Ok(canon_target), Ok(canon_logs)) =
                (target_log_dir.canonicalize(), logs_dir.canonicalize())
            {
                if !canon_target.starts_with(&canon_logs) {
                    return Ok(format!(
                        "Invalid session ID '{}': target path resolves outside session logs directory.",
                        trimmed_arg
                    ));
                }
            }
            let logger = agendex_trajectory::TrajectoryLogger::new(&target_log_dir)?;
            let steps = logger.get_steps().to_vec();
            {
                let mut traj = self.trajectory.lock().await;
                *traj = logger;
            }

            // Reload past conversation messages into self.history
            let mut history = self.history.lock().await;
            let mut reloaded = Vec::new();
            if let Some(sys) = history.first() {
                if sys.role == "system" {
                    reloaded.push(sys.clone());
                }
            } else {
                reloaded.push(ChatMessage::system(
                    "You are Agendex, a local-first autonomous developer agent.",
                ));
            }

            for step in &steps {
                match step.step_type.as_str() {
                    "USER_INPUT" => {
                        reloaded.push(ChatMessage::user(&step.content));
                    }
                    "MODEL_TURN" => {
                        let tool_calls = step.tool_calls.as_ref().and_then(|v| {
                            serde_json::from_value::<Vec<crate::model::ToolCall>>(v.clone()).ok()
                        });
                        reloaded.push(ChatMessage::assistant(
                            if step.content.is_empty() {
                                None
                            } else {
                                Some(step.content.clone())
                            },
                            tool_calls,
                        ));
                    }
                    "TOOL_RESULT" => {
                        let name = step
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("tool_name").or_else(|| m.get("tool")))
                            .and_then(|n| n.as_str())
                            .unwrap_or("tool");
                        let call_id = step
                            .metadata
                            .as_ref()
                            .and_then(|m| {
                                m.get("call_id")
                                    .or_else(|| m.get("id"))
                                    .or_else(|| m.get("tool_call_id"))
                            })
                            .and_then(|c| c.as_str())
                            .unwrap_or("");
                        reloaded.push(ChatMessage::tool_result(call_id, name, &step.content));
                    }
                    _ => {}
                }
            }
            let restored_count = reloaded.len();
            *history = reloaded;

            return Ok(format!(
                "Resumed session '{}'. Trajectory reconnected and {} conversation messages restored.",
                trimmed_arg, restored_count
            ));
        }

        if !logs_dir.exists() {
            return Ok("No past sessions found in state directory.".to_string());
        }

        let mut entries = tokio::fs::read_dir(&logs_dir).await?;
        let mut session_summaries = Vec::new();

        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                let session_name = entry.file_name().to_string_lossy().to_string();
                let traj_file = entry.path().join("trajectory.jsonl");
                let mut preview = "No prompt recorded".to_string();
                let mut step_count = 0;
                if traj_file.exists() {
                    if let Ok(content) = tokio::fs::read_to_string(&traj_file).await {
                        let lines: Vec<&str> = content.lines().collect();
                        step_count = lines.len();
                        for line in &lines {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                                if val.get("type").and_then(|t| t.as_str()) == Some("USER_INPUT") {
                                    if let Some(c) = val.get("content").and_then(|c| c.as_str()) {
                                        preview = c.chars().take(60).collect();
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                session_summaries.push((session_name, step_count, preview));
            }
        }

        if session_summaries.is_empty() {
            return Ok("No recorded sessions found in workspace.".to_string());
        }

        session_summaries.sort_by_key(|a| std::cmp::Reverse(a.1));
        let mut out = String::new();
        out.push_str("Recent Trajectory Sessions in Workspace:\n\n");
        for (name, steps, prev) in session_summaries.iter().take(10) {
            let is_current = name == &self.config.session_id;
            let current_marker = if is_current { " [Active]" } else { "" };
            out.push_str(&format!(
                "• {:<36} ({} steps){} - \"{}\"\n",
                name, steps, current_marker, prev
            ));
        }
        out.push_str("\nTo resume a past session, type: /resume <session_id>");
        Ok(out)
    }
}

/// Identifies a safe split index for context compaction that guarantees
/// tool calls are never separated from their corresponding tool results.
fn find_safe_compaction_boundary(messages: &[ChatMessage], desired_tail: usize) -> usize {
    if messages.len() <= desired_tail + 2 {
        return 2;
    }
    let target = messages.len() - desired_tail;

    // Scan backwards to find a clean user objective (not a tool result)
    for i in (2..=target).rev() {
        if messages[i].role == "user" && messages[i].tool_call_id.is_none() {
            return i;
        }
    }

    // Scan forward if not found
    for (i, msg) in messages.iter().enumerate().skip(target) {
        if msg.role == "user" && msg.tool_call_id.is_none() {
            return i;
        }
    }

    // Fallback: skip past any dangling tool results
    let mut cut = target;
    while cut < messages.len()
        && (messages[cut].role == "tool" || messages[cut].tool_call_id.is_some())
    {
        cut += 1;
    }
    cut
}

fn format_num(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    for (i, c) in chars.into_iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
