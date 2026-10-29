mod support;

use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_conversation_memory_across_turns() -> anyhow::Result<()> {
    let test_dir = std::env::temp_dir().join(format!("agendex_test_mem_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    // Step 1: Turn 1 response from model
    let resp1 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "My favorite color is emerald green."
            }
        }],
        "usage": { "prompt_tokens": 15, "completion_tokens": 10 }
    });

    // Step 2: Turn 2 response from model
    let resp2 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "As I mentioned, my favorite color is emerald green."
            }
        }],
        "usage": { "prompt_tokens": 40, "completion_tokens": 12 }
    });

    let server = MockModelServer::start(vec![resp1, resp2]).await?;

    let agent = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: "test_session_mem".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "dummy".to_string(),
            base_url: Some(server.base_url()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;

    // Turn 1
    let answer1 = agent.process_turn("What is your favorite color?").await?;
    assert!(answer1.contains("emerald green"));

    // Turn 2
    let answer2 = agent
        .process_turn("Can you repeat what you just told me?")
        .await?;
    assert!(answer2.contains("emerald green"));

    // Inspect the second HTTP request to verify history was passed
    let requests = server.requests().await;
    assert_eq!(requests.len(), 2, "Expected 2 HTTP requests to the model");

    let second_req = &requests[1];
    let messages = second_req["messages"].as_array().unwrap();

    // Verify messages contain turn 1 user query, turn 1 assistant answer, and turn 2 query
    let has_first_user = messages
        .iter()
        .any(|m| m["content"] == "What is your favorite color?");
    let has_first_assistant = messages
        .iter()
        .any(|m| m["content"] == "My favorite color is emerald green.");
    let has_second_user = messages
        .iter()
        .any(|m| m["content"] == "Can you repeat what you just told me?");

    assert!(
        has_first_user,
        "Turn 2 request must contain Turn 1 user query"
    );
    assert!(
        has_first_assistant,
        "Turn 2 request must contain Turn 1 assistant reply"
    );
    assert!(
        has_second_user,
        "Turn 2 request must contain Turn 2 user query"
    );

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}

#[tokio::test]
async fn test_session_resume_preserves_tool_calls_and_results() -> anyhow::Result<()> {
    let test_dir =
        std::env::temp_dir().join(format!("agendex_test_resume_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    let sample_file = test_dir.join("sample.txt");
    tokio::fs::write(&sample_file, "Line 1: important content\n").await?;

    // Turn 1 step 1: Model emits tool call to read_file
    let resp1 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Reading the sample file...",
                "tool_calls": [{
                    "id": "tc_read_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": json!({ "path": sample_file.to_string_lossy().to_string() }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 30, "completion_tokens": 15 }
    });

    // Turn 1 step 2: Model finishes turn
    let resp2 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "The file contains important content."
            }
        }],
        "usage": { "prompt_tokens": 60, "completion_tokens": 10 }
    });

    // Turn 2 in resumed agent: Model responds to follow-up
    let resp3 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Understood, continuing from where we left off."
            }
        }],
        "usage": { "prompt_tokens": 90, "completion_tokens": 12 }
    });

    let server = MockModelServer::start(vec![resp1, resp2, resp3]).await?;

    let session_id = format!("resumed_session_{}", uuid::Uuid::new_v4());
    let config = AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: session_id.clone(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "dummy".to_string(),
            base_url: Some(server.base_url()),
            ..Default::default()
        },
        ..Default::default()
    };

    // Agent 1 executes Turn 1 with tool call and result
    {
        let agent1 = AgendexAgent::init(config.clone()).await?;
        let turn1_reply = agent1.process_turn("Check the sample file").await?;
        assert!(turn1_reply.contains("important content"));
    }

    // Agent 2 resumes the session from disk (simulating --conversation <id>)
    let resumed_agent = AgendexAgent::init(config.clone()).await?;

    // Verify history in resumed agent has assistant tool calls before tool results
    {
        let history = resumed_agent.history.lock().await;
        let assistant_with_tc = history
            .iter()
            .find(|m| m.role == "assistant" && m.tool_calls.is_some())
            .expect("Resumed history must contain assistant message with tool calls");

        let tc = assistant_with_tc.tool_calls.as_ref().unwrap();
        assert_eq!(tc[0].id, "tc_read_1");
        assert_eq!(tc[0].name, "read_file");

        let tool_result = history
            .iter()
            .find(|m| m.role == "tool" && m.tool_call_id.as_deref() == Some("tc_read_1"))
            .expect("Resumed history must contain matching tool result");

        assert!(tool_result.content.contains("important content"));
    }

    // Process turn 2 on resumed agent
    let turn2_reply = resumed_agent
        .process_turn("What did you find earlier?")
        .await?;
    assert!(turn2_reply.contains("continuing from where we left off"));

    // Inspect third HTTP request sent to model: tool result must be preceded by assistant tool calls
    let requests = server.requests().await;
    assert_eq!(requests.len(), 3);

    let req3_messages = requests[2]["messages"].as_array().unwrap();
    let tc_idx = req3_messages
        .iter()
        .position(|m| m.get("tool_calls").is_some())
        .expect("Request 3 must contain tool calls in history");
    let tr_idx = req3_messages
        .iter()
        .position(|m| m["role"] == "tool" && m["tool_call_id"] == "tc_read_1")
        .expect("Request 3 must contain tool result");

    assert_eq!(
        tr_idx,
        tc_idx + 1,
        "Tool result must immediately follow the assistant message that requested it"
    );

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}
