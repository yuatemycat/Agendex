use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// High-level discrete event emitted by Agendex during execution turns.
/// Formats as structured JSON lines for headless CI/CD automation and scripts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Fired when an agent begins processing a new user turn.
    TurnStart { session_id: String, prompt: String },
    /// Fired when the model requests execution of a tool.
    ToolCall {
        id: String,
        tool: String,
        arguments: serde_json::Value,
    },
    /// Fired when a tool returns an execution result.
    ToolResult {
        id: String,
        tool: String,
        result: serde_json::Value,
    },
    /// Fired when the final response for the turn has been generated.
    FinalOutput { text: String },
    /// Fired when an error occurs during turn execution.
    Error { message: String },
    /// Fired when automated git commit operation occurs.
    GitCommit { commit_sha: String, message: String },
    /// Fired when automated git branch creation or checkout occurs.
    GitBranch { branch: String },
}

/// Callback handler type for listening to runtime AgentEvents.
pub type AgentEventHandler = Arc<dyn Fn(&AgentEvent) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_json_serialization() {
        let turn_start = AgentEvent::TurnStart {
            session_id: "sess_1".to_string(),
            prompt: "Refactor router".to_string(),
        };
        let line = serde_json::to_string(&turn_start).unwrap();
        assert!(line.contains(r#""event":"turn_start""#));
        assert!(line.contains(r#""prompt":"Refactor router""#));

        let tool_call = AgentEvent::ToolCall {
            id: "call_1".to_string(),
            tool: "find_definition".to_string(),
            arguments: serde_json::json!({ "symbol": "Engine" }),
        };
        let line = serde_json::to_string(&tool_call).unwrap();
        assert!(line.contains(r#""event":"tool_call""#));
        assert!(line.contains(r#""tool":"find_definition""#));

        let tool_res = AgentEvent::ToolResult {
            id: "call_1".to_string(),
            tool: "find_definition".to_string(),
            result: serde_json::json!({ "total_found": 1 }),
        };
        let line = serde_json::to_string(&tool_res).unwrap();
        assert!(line.contains(r#""event":"tool_result""#));

        let final_out = AgentEvent::FinalOutput {
            text: "Task completed successfully.".to_string(),
        };
        let line = serde_json::to_string(&final_out).unwrap();
        assert!(line.contains(r#""event":"final_output""#));

        let git_evt = AgentEvent::GitCommit {
            commit_sha: "abcdef123456".to_string(),
            message: "agendex: completed task".to_string(),
        };
        let line = serde_json::to_string(&git_evt).unwrap();
        assert!(line.contains(r#""event":"git_commit""#));
    }
}
