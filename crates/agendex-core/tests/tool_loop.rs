mod support;

use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_agent_tool_loop_execution() -> anyhow::Result<()> {
    let test_dir = std::env::temp_dir().join(format!("agendex_test_loop_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    let file_to_read = test_dir.join("sample.txt");
    tokio::fs::write(&file_to_read, "CONTENT_OF_SAMPLE_TXT").await?;

    // Step 1: Model asks to read sample.txt
    let resp1 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc_read_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": json!({
                            "path": "sample.txt"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
    });

    // Step 2: Model receives tool result and produces terminal answer
    let resp2 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "The file contains CONTENT_OF_SAMPLE_TXT"
            }
        }],
        "usage": { "prompt_tokens": 120, "completion_tokens": 15 }
    });

    let server = MockModelServer::start(vec![resp1, resp2]).await?;

    let agent = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: "test_session_loop".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "test_key".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(4096),
        },
        max_tool_iterations: 10,
        ..Default::default()
    })
    .await?;

    let reply = agent.process_turn("Please inspect sample.txt").await?;
    assert!(reply.contains("The file contains CONTENT_OF_SAMPLE_TXT"));

    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);

    // Verify the second request passed the tool result back
    let second_req = &requests[1];
    let messages = second_req["messages"].as_array().unwrap();
    let has_tool_resp = messages.iter().any(|m| {
        m.get("role").and_then(|r| r.as_str()) == Some("tool")
            && m.get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.contains("CONTENT_OF_SAMPLE_TXT"))
                .unwrap_or(false)
    });
    assert!(has_tool_resp, "Second request must contain tool result");

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}
