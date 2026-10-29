mod support;

use agendex_core::{ChatMessage, ModelClient, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_wire_anthropic_format() -> anyhow::Result<()> {
    let mock_resp = json!({
        "content": [
            {
                "type": "text",
                "text": "I will examine the codebase."
            },
            {
                "type": "tool_use",
                "id": "toolu_01A",
                "name": "list_directory",
                "input": { "path": "." }
            }
        ],
        "usage": {
            "input_tokens": 85,
            "output_tokens": 28
        }
    });

    let server = MockModelServer::start(vec![mock_resp]).await?;

    let client = ModelClient::new(ModelConfig {
        format: ModelFormat::Anthropic,
        model_name: "claude-3-5-sonnet-20241022".to_string(),
        api_key: "ant_test_key".to_string(),
        base_url: Some(server.base_url()),
        temperature: Some(0.3),
        max_output_tokens: Some(4096),
    });

    let messages = vec![
        ChatMessage::system("You are Claude."),
        ChatMessage::user("List current directory"),
    ];

    let tools = vec![json!({
        "name": "list_directory",
        "description": "List files",
        "parameters": { "type": "object" }
    })];

    let resp = client.chat(&messages, &tools).await?;

    assert_eq!(resp.text.as_deref(), Some("I will examine the codebase."));
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "toolu_01A");
    assert_eq!(resp.tool_calls[0].name, "list_directory");
    assert_eq!(resp.tool_calls[0].arguments["path"], ".");
    assert_eq!(resp.prompt_tokens, 85);
    assert_eq!(resp.completion_tokens, 28);

    let recorded = server.requests().await;
    assert_eq!(recorded.len(), 1);
    let req = &recorded[0];
    assert_eq!(req["model"], "claude-3-5-sonnet-20241022");
    assert_eq!(req["system"], "You are Claude.");
    assert!(req["tools"].is_array());

    Ok(())
}
