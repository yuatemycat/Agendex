pub mod agent;
pub mod battle;
pub mod config;
pub mod events;
pub mod git;
pub mod mailbox;
pub mod model;
pub mod safety;
pub mod swarm;
pub mod worktree;

pub use agent::{AgendexAgent, AgentConfig};
pub use battle::{
    BattleArena, BattleCombatantResult, BattleDuelConfig, BattleReport, BattleVerdict,
};
pub use config::{
    default_config_path, load_config_from_path, load_config_from_str, load_default_config,
    resolve_provider_preset, AgendexConfigFile,
};
pub use events::{AgentEvent, AgentEventHandler};
pub use git::{
    commit_task_changes, create_or_checkout_branch, ensure_git_identity, get_uncommitted_files,
    GitCommitResult,
};
pub use mailbox::{MailboxEvent, MailboxManager, MailboxMessage, MessagePriority};
pub use model::{
    ChatMessage, ModelClient, ModelConfig, ModelFormat, ModelResponse, TokenStreamHandler, ToolCall,
};
pub use safety::{CommandSafetyAssessor, CommandSafetyVerdict};
pub use swarm::{SubagentInstance, SubagentManager, SubagentPersona, SubagentStatus};
pub use worktree::{IsolatedWorkspace, WorkspaceMode, WorktreeManager};
