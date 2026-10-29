mod support;

use agendex_core::{ChatMessage, ModelClient, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_wire_openai_format() -> anyhow::Result<()> {
    let mock_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_12345",
                    "type": "function",
                    "function": {
                        "name": "edit_file",
                        "arguments": json!({
                            "path": "src/lib.rs",
                            "target": "old",
                            "replacement": "new"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": {
            "prompt_tokens": 120,
            "completion_tokens": 35
        }
    });

    let server = MockModelServer::start(vec![mock_resp]).await?;

    let client = ModelClient::new(ModelConfig {
        format: ModelFormat::OpenAi,
        model_name: "gpt-4o".to_string(),
        api_key: "test_key".to_string(),
        base_url: Some(server.base_url()),
        temperature: Some(0.0),
        max_output_tokens: Some(4096),
    });

    let messages = vec![
        ChatMessage::system("You are an assistant."),
        ChatMessage::user("Update lib.rs"),
    ];

    let tools = vec![json!({
        "name": "edit_file",
        "description": "Edit file",
        "parameters": { "type": "object" }
    })];

    let resp = client.chat(&messages, &tools).await?;

    assert_eq!(resp.text, None);
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "call_12345");
    assert_eq!(resp.tool_calls[0].name, "edit_file");
    assert_eq!(resp.tool_calls[0].arguments["replacement"], "new");
    assert_eq!(resp.prompt_tokens, 120);
    assert_eq!(resp.completion_tokens, 35);

    let recorded = server.requests().await;
    assert_eq!(recorded.len(), 1);
    let req = &recorded[0];
    assert_eq!(req["model"], "gpt-4o");
    assert!(req["messages"].is_array());
    assert!(req["tools"].is_array());

    Ok(())
}
