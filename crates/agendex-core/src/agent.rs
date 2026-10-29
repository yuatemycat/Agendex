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

pub struct AgendexAgent {
    pub config: AgentConfig,
    pub model_client: Arc<ModelClient>,
    pub task_manager: Arc<TaskManager>,
    pub tool_registry: Arc<ToolRegistry>,
    pub trajectory: Arc<Mutex<TrajectoryLogger>>,
    pub compactor: Arc<Mutex<CompactorEngine>>,
    pub snapshot_engine: Arc<Mutex<SnapshotEngine>>,
    pub skill_registry: Arc<SkillRegistry>,
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
}

impl AgendexAgent {
    pub async fn init(config: AgentConfig) -> anyhow::Result<Self> {
        // Initialize SnapshotEngine first (ensures git and state_dir are initialized consistently)
        let snapshot_engine_raw = SnapshotEngine::new(&config.workspace_dir)?;
        let state_dir = snapshot_engine_raw.state_dir().to_path_buf();
        let log_dir = state_dir.join("logs").join(&config.session_id);
        tokio::fs::create_dir_all(&log_dir).await?;

        let task_manager = Arc::new(TaskManager::new(state_dir.join("tasks")));
        let tool_registry = Arc::new(ToolRegistry::new(
            config.workspace_dir.clone(),
            Arc::clone(&task_manager),
        ));
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
            skill_registry: Arc::new(skill_registry),
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
        })
    }

    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
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
        if let Some(handler) = &self.approval_handler {
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
        // 1. Check for slash command resolution
        if user_input.starts_with('/') {
            let cmd = user_input.split_whitespace().next().unwrap_or("");
            match cmd {
                "/help" => {
                    return Ok(self.skill_registry.format_slash_commands_prompt());
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
                "/revert" => {
                    let mut snap = self.snapshot_engine.lock().await;
                    snap.rollback_latest().await?;
                    return Ok("Reverted workspace to latest snapshot checkpoint.".to_string());
                }
                _ => {
                    if let Some(skill) = self.skill_registry.get_skill_by_command(cmd) {
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
        {
            let mut snap = self.snapshot_engine.lock().await;
            let _ = snap.capture_snapshot("pre-turn").await;
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

        // 6. Access and update conversation history across turns
        let mut history = self.history.lock().await;

        if history.is_empty() {
            let system_prompt = format!(
                "You are Agendex, a powerful local-first autonomous agent.\n\
                Workspace: {}\n\
                Available Slash Commands:\n{}\n\
                Active Rules:\n{}",
                self.config.workspace_dir.display(),
                self.skill_registry.format_slash_commands_prompt(),
                self.rules_engine.generate_rule_blocks(None)
            );
            history.push(ChatMessage::system(system_prompt));
        }

        history.push(ChatMessage::user(user_input));

        let mut turn_iteration = 0;
        let mut total_tokens = 0;

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
            let resp = self.model_client.chat(&history, &tools).await?;

            total_tokens = resp.prompt_tokens + resp.completion_tokens;

            // If the model emitted tool calls, execute them
            if !resp.tool_calls.is_empty() {
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

                for tc in resp.tool_calls {
                    let tool_result = self.dispatch_tool(&tc).await;

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
                self.hook_dispatcher
                    .trigger_post_tool("turn_complete", "success")
                    .await?;
                return Ok(final_text);
            }
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
                    let approved = if let Some(handler) = &self.approval_handler {
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
        if tc.name == "write_file" || tc.name == "edit_file" {
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

                let target_path = self.config.workspace_dir.join(path);
                match self
                    .ast_patcher
                    .patch_file(&target_path, target, replacement)
                {
                    Ok(res) => format!(
                        "Successfully replaced {} occurrences in {}",
                        res.occurrences, path
                    ),
                    Err(e) => format!("Failed to edit {}: {}", path, e),
                }
            }
            _ => {
                match self
                    .tool_registry
                    .execute_tool(&tc.name, tc.arguments.clone())
                    .await
                {
                    Ok(res) => {
                        serde_json::to_string_pretty(&res).unwrap_or_else(|_| "{}".to_string())
                    }
                    Err(e) => format!("Tool execution error for '{}': {}", tc.name, e),
                }
            }
        }
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
