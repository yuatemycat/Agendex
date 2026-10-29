mod support;

use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_iteration_cap_halts_loop() -> anyhow::Result<()> {
    let test_dir = std::env::temp_dir().join(format!("agendex_test_cap_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    // Create an endless sequence of tool calls
    let loop_call = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc_repeat",
                    "type": "function",
                    "function": {
                        "name": "list_directory",
                        "arguments": json!({ "path": "." }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 10 }
    });

    let responses = vec![loop_call.clone(); 10];
    let server = MockModelServer::start(responses).await?;

    let agent = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: "test_session_cap".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "dummy".to_string(),
            base_url: Some(server.base_url()),
            ..Default::default()
        },
        max_tool_iterations: 3, // Bound loop to 3
        ..Default::default()
    })
    .await?;

    let reply = agent.process_turn("Infinite loop test").await?;
    assert!(
        reply.contains("maximum tool execution iterations")
            || reply.contains("Maximum tool iterations"),
        "Should return iteration cap message, got: {}",
        reply
    );

    let requests = server.requests().await;
    assert_eq!(requests.len(), 3);

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}
