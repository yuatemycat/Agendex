mod support;

use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use serde_json::json;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_model_http_error_returns_err() -> anyhow::Result<()> {
    let err_resp = json!({
        "error": {
            "message": "Incorrect API key provided",
            "type": "invalid_request_error"
        }
    });

    let server = MockModelServer::start_with_status(401, vec![err_resp]).await?;

    let test_dir = std::env::temp_dir().join(format!("agendex_test_err_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    let agent = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: "test_session_err".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "bad_key".to_string(),
            base_url: Some(server.base_url()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?;

    let result = agent.process_turn("Hello world").await;
    assert!(result.is_err(), "HTTP error must return Err");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("OpenAI API error") && err_msg.contains("Incorrect API key provided"),
        "Error message should report API error and details, got: {}",
        err_msg
    );

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}
