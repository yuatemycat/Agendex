mod support;

use agendex_core::{ChatMessage, ModelClient, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_wire_gemini_format() -> anyhow::Result<()> {
    let mock_resp = json!({
        "candidates": [{
            "content": {
                "parts": [
                    { "text": "Analyzing code..." },
                    {
                        "functionCall": {
                            "name": "read_file",
                            "args": { "path": "src/main.rs" }
                        }
                    }
                ],
                "role": "model"
            }
        }],
        "usageMetadata": {
            "promptTokenCount": 42,
            "candidatesTokenCount": 18
        }
    });

    let server = MockModelServer::start(vec![mock_resp]).await?;

    let client = ModelClient::new(ModelConfig {
        format: ModelFormat::GoogleGemini,
        model_name: "gemini-2.5-flash".to_string(),
        api_key: "dummy_key".to_string(),
        base_url: Some(server.base_url()),
        temperature: Some(0.2),
        max_output_tokens: Some(4096),
    });

    let messages = vec![
        ChatMessage::system("You are a helpful coding assistant."),
        ChatMessage::system("[COMPACTED STATE]: Earlier turn summarized."),
        ChatMessage::user("Please read src/main.rs"),
    ];

    let tools = vec![json!({
        "name": "read_file",
        "description": "Read file contents",
        "parameters": {
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        }
    })];

    let resp = client.chat(&messages, &tools).await?;

    assert_eq!(resp.text.as_deref(), Some("Analyzing code..."));
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].name, "read_file");
    assert_eq!(resp.tool_calls[0].arguments["path"], "src/main.rs");
    assert_eq!(resp.prompt_tokens, 42);
    assert_eq!(resp.completion_tokens, 18);

    let recorded = server.requests().await;
    assert_eq!(recorded.len(), 1);
    let req = &recorded[0];
    assert!(req.get("contents").is_some());
    assert!(req.get("tools").is_some());

    let sys_text = req["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(sys_text.contains("helpful coding assistant"));
    assert!(sys_text.contains("[COMPACTED STATE]"));

    Ok(())
}
