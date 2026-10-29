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
    pub model_client: Arc<ModelClient>,
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
    pub touched_files: Arc<Mutex<std::collections::HashSet<String>>>,
    pub streamed_tokens_in_turn: Arc<std::sync::atomic::AtomicBool>,
}

impl AgendexAgent {
    pub async fn init(config: AgentConfig) -> anyhow::Result<Self> {
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
        let model_client = Arc::new(ModelClient::new(config.model.clone()));

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
            touched_files: Arc::new(Mutex::new(std::collections::HashSet::new())),
            streamed_tokens_in_turn: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }

    pub fn streamed_tokens_in_last_turn(&self) -> bool {
        self.streamed_tokens_in_turn
            .load(std::sync::atomic::Ordering::SeqCst)
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

    pub fn notify_progress(&self, msg: &str) {
        if let Some(handler) = &self.tool_progress_handler {
            handler(msg);
        }
    }

    pub async fn request_diff_approval(&self, path: &str, diff: &str) -> bool {
        if self.config.auto_approve_risky {
            return true;
        }
        if let Some(handler) = &self.diff_approval_handler {
            return handler(path, diff);
        }
        if let Some(queue) = &self.approval_queue {
            let agent_id = self.agent_id.as_deref().unwrap_or("parent");
            return queue.request_diff_approval(agent_id, path, diff).await;
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
                return trimmed == "y" || trimmed == "yes";
            }
            return false;
        }
        // Non-terminal execution without -y refuses edits, matching risky command policy
        false
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

    pub fn with_model_client(mut self, model_client: Arc<ModelClient>) -> Self {
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
        self.streamed_tokens_in_turn
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.emit_event(crate::events::AgentEvent::TurnStart {
            session_id: self.config.session_id.clone(),
            prompt: user_input.to_string(),
        });

        // 1. Check for slash command resolution
        if user_input.starts_with('/') {
            let cmd = user_input.split_whitespace().next().unwrap_or("");
            match cmd {
                "/help" => {
                    return Ok(self
                        .skill_registry
                        .read()
                        .map(|r| r.format_slash_commands_prompt())
                        .unwrap_or_default());
                }
                "/compact" => {
                    let mut comp = self.compactor.lock().await;
                    let summary =
                        comp.generate_continuation_summary("Manual context compaction triggered.");
                    return Ok(format!(
                        "Context compacted.\n\nContinuation Summary:\n{}",
                        summary
                    ));
                }
                "/def" | "/definition" => {
                    let sym = user_input.split_whitespace().nth(1).unwrap_or("");
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
                "/revert" => {
                    let mut snap = self.snapshot_engine.lock().await;
                    match snap.rollback_turn().await? {
                        Some(res) => {
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
                        None => {
                            return Ok("No previous turn snapshot available to revert.".to_string());
                        }
                    }
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
            let stream_handler: Option<TokenStreamHandler> =
                if let Some(handler) = &self.token_stream_handler {
                    let flag = Arc::clone(&self.streamed_tokens_in_turn);
                    let h = Arc::clone(handler);
                    Some(Arc::new(move |token: &str| {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        h(token);
                    }))
                } else {
                    None
                };
            let resp = self
                .model_client
                .chat_with_stream(&history, &tools, stream_handler)
                .await?;

            turn_prompt_tokens += resp.prompt_tokens;
            turn_completion_tokens += resp.completion_tokens;
            total_tokens = resp.prompt_tokens + resp.completion_tokens;

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
                                serde_json::json!({ "tool": tc.name, "id": tc.id }),
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
                    );
                }

                self.hook_dispatcher
                    .trigger_post_tool("turn_complete", "success")
                    .await?;
                return Ok(final_text);
            }
        }

        // Record token usage if maximum iterations reached
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
            );
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
                    let approved = if let Some(queue) = &self.approval_queue {
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
                    let approved = self.request_diff_approval(path, &diff).await;
                    if !approved {
                        self.notify_progress(&format!(
                            "[Tool: edit_file] Edit on '{}' rejected by user.",
                            path
                        ));
                        return "Edit rejected by user".to_string();
                    }
                }

                let result = match self
                    .ast_patcher
                    .patch_file(&target_path, target, replacement)
                {
                    Ok(res) => {
                        self.touched_files
                            .lock()
                            .await
                            .insert(path.replace('\\', "/"));
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
                    format!("{}...", content[..400].trim())
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
                } else {
                    self.config.auto_approve_risky
                };

                if !approved {
                    tracing::warn!(
                        "Safety gate rejected save_skill for '{}': {}",
                        safe_name,
                        reason
                    );
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
                                    let approved =
                                        self.request_diff_approval(path_str, &diff).await;
                                    if !approved {
                                        self.notify_progress(&format!(
                                            "[Tool: write_file] Edit on '{}' rejected by user.",
                                            path_str
                                        ));
                                        return "Edit rejected by user".to_string();
                                    }
                                }
                            }
                        } else if !target_path.exists() {
                            // Review diff for proposed new file creation
                            let diff =
                                agendex_ast::diff::generate_unified_diff(path_str, "", content);
                            if !diff.is_empty() {
                                let approved = self.request_diff_approval(path_str, &diff).await;
                                if !approved {
                                    self.notify_progress(&format!(
                                        "[Tool: write_file] File creation of '{}' rejected by user.",
                                        path_str
                                    ));
                                    return "File creation rejected by user".to_string();
                                }
                            }
                        }
                    }
                }

                let pre_cmd_untracked = if tc.name == "run_command" {
                    let out = tokio::process::Command::new("git")
                        .args(["ls-files", "--others", "--exclude-standard"])
                        .current_dir(&self.config.workspace_dir)
                        .output()
                        .await;
                    out.ok().map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .map(|l| l.trim().replace('\\', "/"))
                            .filter(|l| !l.is_empty())
                            .collect::<Vec<String>>()
                    })
                } else {
                    None
                };

                let res_val = self
                    .tool_registry
                    .execute_tool(&tc.name, tc.arguments.clone())
                    .await;

                if let Some(pre_files) = pre_cmd_untracked {
                    let post_out = tokio::process::Command::new("git")
                        .args(["ls-files", "--others", "--exclude-standard"])
                        .current_dir(&self.config.workspace_dir)
                        .output()
                        .await;
                    if let Ok(o) = post_out {
                        let mut snap = self.snapshot_engine.lock().await;
                        let mut touched = self.touched_files.lock().await;
                        for line in String::from_utf8_lossy(&o.stdout).lines() {
                            let trimmed = line.trim().replace('\\', "/");
                            if !trimmed.is_empty() && !pre_files.contains(&trimmed) {
                                snap.record_created_file(&trimmed);
                                touched.insert(trimmed);
                            }
                        }
                    }
                }

                match res_val {
                    Ok(res) => {
                        if tc.name == "write_file" {
                            if let Some(path_str) = res.get("path").and_then(|p| p.as_str()) {
                                self.touched_files
                                    .lock()
                                    .await
                                    .insert(path_str.replace('\\', "/"));
                                if res
                                    .get("created")
                                    .and_then(|c| c.as_bool())
                                    .unwrap_or(false)
                                {
                                    let mut snap = self.snapshot_engine.lock().await;
                                    snap.record_created_file(path_str);
                                }
                            }
                        }
                        self.notify_progress(&format!("[Tool: {}] Completed.", tc.name));
                        serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string())
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
