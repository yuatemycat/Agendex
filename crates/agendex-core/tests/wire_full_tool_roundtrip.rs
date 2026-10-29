mod support;

use agendex_core::{ChatMessage, ModelClient, ModelConfig, ModelFormat, ToolCall};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_gemini_full_tool_roundtrip_formatting() -> anyhow::Result<()> {
    // Model responds with final confirmation after receiving functionResponse
    let mock_resp = json!({
        "candidates": [{
            "content": {
                "parts": [{ "text": "Successfully read file content." }],
                "role": "model"
            }
        }],
        "usageMetadata": { "promptTokenCount": 50, "candidatesTokenCount": 10 }
    });

    let server = MockModelServer::start(vec![mock_resp]).await?;

    let client = ModelClient::new(ModelConfig {
        format: ModelFormat::GoogleGemini,
        model_name: "gemini-2.5-flash".to_string(),
        api_key: "dummy".to_string(),
        base_url: Some(server.base_url()),
        ..Default::default()
    });

    // Simulate multi-turn tool interaction:
    // 1. System prompt
    // 2. User prompt
    // 3. Assistant functionCall
    // 4. Tool functionResponse
    let messages = vec![
        ChatMessage::system("System instructions for Agendex"),
        ChatMessage::user("Read cargo.toml"),
        ChatMessage::assistant(
            None,
            Some(vec![ToolCall {
                id: "call_abc".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "Cargo.toml" }),
            }]),
        ),
        ChatMessage::tool_result("call_abc", "read_file", "[package]\nname = \"agendex\""),
    ];

    let tools = vec![json!({
        "name": "read_file",
        "description": "Reads a file",
        "parameters": { "type": "object" }
    })];

    let resp = client.chat(&messages, &tools).await?;
    assert_eq!(
        resp.text.as_deref(),
        Some("Successfully read file content.")
    );

    let recorded = server.requests().await;
    assert_eq!(recorded.len(), 1);

    let req = &recorded[0];

    // Assert systemInstruction is at top level
    assert!(
        req.get("systemInstruction").is_some(),
        "Gemini payload must use top-level systemInstruction"
    );
    assert_eq!(
        req["systemInstruction"]["parts"][0]["text"],
        "System instructions for Agendex"
    );

    // Assert contents contains functionCall and functionResponse
    let contents = req["contents"].as_array().unwrap();
    let has_func_call = contents.iter().any(|c| {
        c["parts"]
            .as_array()
            .map(|parts| parts.iter().any(|p| p.get("functionCall").is_some()))
            .unwrap_or(false)
    });
    let has_func_resp = contents.iter().any(|c| {
        c["parts"]
            .as_array()
            .map(|parts| parts.iter().any(|p| p.get("functionResponse").is_some()))
            .unwrap_or(false)
    });

    assert!(
        has_func_call,
        "Gemini payload must contain assistant functionCall"
    );
    assert!(
        has_func_resp,
        "Gemini payload must contain tool functionResponse, NOT plain text"
    );

    // Verify functionResponse structure
    let resp_part = contents
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .find(|p| p.get("functionResponse").is_some())
        .unwrap();
    assert_eq!(resp_part["functionResponse"]["name"], "read_file");

    Ok(())
}

#[tokio::test]
async fn test_anthropic_full_tool_roundtrip_formatting() -> anyhow::Result<()> {
    let mock_resp = json!({
        "content": [{ "type": "text", "text": "File read complete." }],
        "usage": { "input_tokens": 60, "output_tokens": 12 }
    });

    let server = MockModelServer::start(vec![mock_resp]).await?;

    let client = ModelClient::new(ModelConfig {
        format: ModelFormat::Anthropic,
        model_name: "claude-3-5-sonnet-20241022".to_string(),
        api_key: "dummy".to_string(),
        base_url: Some(server.base_url()),
        ..Default::default()
    });

    // Simulate multi-turn tool interaction:
    // 1. System prompt
    // 2. User prompt
    // 3. Assistant tool_use
    // 4. Tool tool_result
    let messages = vec![
        ChatMessage::system("Anthropic system instructions"),
        ChatMessage::user("Inspect config"),
        ChatMessage::assistant(
            None,
            Some(vec![ToolCall {
                id: "toolu_xyz".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "config.json" }),
            }]),
        ),
        ChatMessage::tool_result("toolu_xyz", "read_file", "{\"port\": 8080}"),
    ];

    let tools = vec![json!({
        "name": "read_file",
        "description": "Reads file",
        "parameters": { "type": "object" }
    })];

    let resp = client.chat(&messages, &tools).await?;
    assert_eq!(resp.text.as_deref(), Some("File read complete."));

    let recorded = server.requests().await;
    assert_eq!(recorded.len(), 1);

    let req = &recorded[0];

    // Assert top-level system prompt
    assert_eq!(req["system"], "Anthropic system instructions");

    // Inspect messages
    let msgs = req["messages"].as_array().unwrap();

    // Find the tool response message
    let tool_msg = msgs
        .iter()
        .find(|m| {
            m["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                })
                .unwrap_or(false)
        })
        .expect("Anthropic request must contain a tool_result message");

    let blocks = tool_msg["content"].as_array().unwrap();

    // Verify there is NO text block preceding tool_result!
    let has_text_block = blocks
        .iter()
        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"));
    assert!(
        !has_text_block,
        "Anthropic tool message must NOT have an extraneous text block placed before tool_result"
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(blocks[0]["tool_use_id"], "toolu_xyz");

    Ok(())
}
