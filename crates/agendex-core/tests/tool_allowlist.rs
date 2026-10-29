mod support;

use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_child_tool_allowlist_enforcement() -> anyhow::Result<()> {
    let test_root = std::env::temp_dir().join(format!("agendex_test_w3_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_root).await?;

    // Model attempts to invoke `run_command`, which is NOT in its allowlist
    let resp_turn1 = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc_forbidden_1",
                    "type": "function",
                    "function": {
                        "name": "run_command",
                        "arguments": serde_json::json!({
                            "command_line": "echo SHOULD_NOT_RUN"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 15 }
    });

    let resp_turn2 = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Model acknowledged tool error."
            }
        }],
        "usage": { "prompt_tokens": 25, "completion_tokens": 8 }
    });

    let server = MockModelServer::start(vec![resp_turn1, resp_turn2]).await?;

    let config = AgentConfig {
        workspace_dir: test_root.clone(),
        session_id: "test_allowlist_agent".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "dummy".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(1024),
        },
        ..Default::default()
    };

    let mut agent = AgendexAgent::init(config).await?;
    // Strict allowlist: ONLY read_file is permitted
    agent = agent.with_allowed_tools(vec!["read_file".to_string()]);

    let final_output = agent
        .process_turn("Execute an unauthorized command")
        .await?;
    assert_eq!(final_output, "Model acknowledged tool error.");

    // Verify tools sent over wire to the model in turn 1
    let requests = server.recorded_requests.lock().await;
    assert!(!requests.is_empty());
    let first_req = &requests[0];
    let tools = first_req.get("tools").and_then(|t| t.as_array()).unwrap();

    // Exactly one tool must be exposed: `read_file`
    assert_eq!(tools.len(), 1);
    let tool_name = tools[0]
        .pointer("/function/name")
        .and_then(|n| n.as_str())
        .unwrap();
    assert_eq!(tool_name, "read_file");

    // Verify turn 2 tool result contained the expected security rejection
    let second_req = &requests[1];
    let messages = second_req
        .get("messages")
        .and_then(|m| m.as_array())
        .unwrap();
    let tool_msg = messages
        .iter()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .unwrap();
    let tool_content = tool_msg.get("content").and_then(|c| c.as_str()).unwrap();

    assert!(
        tool_content
            .contains("Tool execution error for 'run_command': Tool is not in allowed tools list"),
        "Unexpected tool response content: {}",
        tool_content
    );

    let _ = tokio::fs::remove_dir_all(&test_root).await;
    Ok(())
}
