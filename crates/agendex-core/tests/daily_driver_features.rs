mod support;

use std::sync::Arc;

use agendex_core::config::{load_config_from_str, resolve_provider_preset};
use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use agendex_trajectory::compactor::ModelTier;
use serde_json::json;
use support::mock_model::MockModelServer;

// ============================================================================
// 1. Config File Support & Proxymateur Preset Tests
// ============================================================================

#[test]
fn test_config_toml_full_parsing() {
    let toml = r#"
        model = "claude-3-5-sonnet-20241022"
        provider = "anthropic"
        base_url = "https://custom.anthropic.endpoint"
        api_key = "sk-test-key-123"
        tier = "256k"
        auto_approve_risky = true
    "#;

    let cfg = load_config_from_str(toml).expect("Valid config toml");
    assert_eq!(cfg.resolved_model(), Some("claude-3-5-sonnet-20241022"));
    assert_eq!(cfg.resolved_provider_or_format(), Some("anthropic"));
    assert_eq!(
        cfg.resolved_base_url(),
        Some("https://custom.anthropic.endpoint")
    );
    assert_eq!(cfg.resolved_api_key(), Some("sk-test-key-123"));
    assert_eq!(cfg.resolved_tier(), Some("256k"));
    assert_eq!(cfg.resolved_auto_approve_risky(), Some(true));

    let mut agent_cfg = AgentConfig::default();
    cfg.apply_to_agent_config(&mut agent_cfg);

    assert_eq!(agent_cfg.model.model_name, "claude-3-5-sonnet-20241022");
    assert_eq!(agent_cfg.model.format, ModelFormat::Anthropic);
    assert_eq!(
        agent_cfg.model.base_url.as_deref(),
        Some("https://custom.anthropic.endpoint")
    );
    assert_eq!(agent_cfg.model.api_key, "sk-test-key-123");
    assert_eq!(agent_cfg.model_tier, ModelTier::HighTier256k);
    assert!(agent_cfg.auto_approve_risky);
}

#[test]
fn test_config_proxymateur_preset_defaults() {
    let toml = r#"
        provider = "proxymateur"
        model = "gpt-4o"
    "#;

    let cfg = load_config_from_str(toml).expect("Valid proxymateur config");
    assert_eq!(cfg.resolved_provider_or_format(), Some("proxymateur"));

    let (format, default_base) = resolve_provider_preset("proxymateur");
    assert_eq!(format, ModelFormat::OpenAi);
    assert_eq!(default_base, Some("http://127.0.0.1:8045/v1".to_string()));

    let mut agent_cfg = AgentConfig::default();
    cfg.apply_to_agent_config(&mut agent_cfg);

    assert_eq!(agent_cfg.model.format, ModelFormat::OpenAi);
    assert_eq!(
        agent_cfg.model.base_url.as_deref(),
        Some("http://127.0.0.1:8045/v1")
    );
    assert_eq!(agent_cfg.model.model_name, "gpt-4o");
}

#[test]
fn test_config_file_loading_from_disk() {
    let tmp_dir = std::env::temp_dir().join(format!("agendex_cfg_disk_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let cfg_file = tmp_dir.join("config.toml");
    std::fs::write(
        &cfg_file,
        r#"
            model = "gemini-2.5-flash"
            format = "google"
            tier = "840k"
            auto_approve_risky = true
        "#,
    )
    .unwrap();

    let cfg = agendex_core::config::load_config_from_path(&cfg_file).unwrap();
    assert_eq!(cfg.resolved_model(), Some("gemini-2.5-flash"));
    assert_eq!(cfg.resolved_provider_or_format(), Some("google"));

    let mut agent_cfg = AgentConfig::default();
    cfg.apply_to_agent_config(&mut agent_cfg);

    assert_eq!(agent_cfg.model.model_name, "gemini-2.5-flash");
    assert_eq!(agent_cfg.model.format, ModelFormat::GoogleGemini);
    assert_eq!(agent_cfg.model_tier, ModelTier::Extended840k);
    assert!(agent_cfg.auto_approve_risky);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

// ============================================================================
// 2. Interactive Diffs & Accept/Reject Before Edits Tests
// ============================================================================

#[tokio::test]
async fn test_interactive_diff_acceptance_applies_edit() -> anyhow::Result<()> {
    let tmp_workspace =
        std::env::temp_dir().join(format!("agendex_diff_acc_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_workspace).await?;
    let test_file = tmp_workspace.join("hello.rs");
    tokio::fs::write(&test_file, "fn greeting() {\n    println!(\"Hello\");\n}\n").await?;

    let mock_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc_edit_1",
                    "type": "function",
                    "function": {
                        "name": "edit_file",
                        "arguments": json!({
                            "path": "hello.rs",
                            "target": "println!(\"Hello\");",
                            "replacement": "println!(\"Hello, world!\");"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
    });

    let final_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Greeting updated successfully."
            }
        }],
        "usage": { "prompt_tokens": 60, "completion_tokens": 10 }
    });

    let server = MockModelServer::start(vec![mock_resp, final_resp]).await?;

    let config = AgentConfig {
        workspace_dir: tmp_workspace.clone(),
        model_tier: ModelTier::Standard128k,
        auto_compaction: false,
        session_id: "diff_accept_test".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "test".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(1024),
        },
        max_tool_iterations: 5,
        mcp_config: None,
        auto_approve_risky: false,
        max_turn_tokens: None,
    };

    let diff_received = Arc::new(std::sync::Mutex::new(String::new()));
    let diff_clone = Arc::clone(&diff_received);

    let agent = AgendexAgent::init(config)
        .await?
        .with_diff_approval_handler(Arc::new(move |_path, diff| {
            let mut d = diff_clone.lock().unwrap();
            *d = diff.to_string();
            true // Accept edit [y]
        }));

    let result = agent
        .process_turn("Update greeting to Hello, world!")
        .await?;
    assert_eq!(result, "Greeting updated successfully.");

    // Verify diff was generated and inspected
    let captured_diff = diff_received.lock().unwrap().clone();
    assert!(captured_diff.contains("--- a/hello.rs"));
    assert!(captured_diff.contains("+++ b/hello.rs"));
    assert!(captured_diff.contains("-    println!(\"Hello\");"));
    assert!(captured_diff.contains("+    println!(\"Hello, world!\");"));

    // Verify file on disk was modified
    let disk_content = tokio::fs::read_to_string(&test_file).await?;
    assert!(disk_content.contains("println!(\"Hello, world!\");"));

    let _ = tokio::fs::remove_dir_all(&tmp_workspace).await;
    Ok(())
}

#[tokio::test]
async fn test_interactive_diff_rejection_leaves_file_untouched() -> anyhow::Result<()> {
    let tmp_workspace =
        std::env::temp_dir().join(format!("agendex_diff_rej_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_workspace).await?;
    let test_file = tmp_workspace.join("hello.rs");
    let original_content = "fn greeting() {\n    println!(\"Original\");\n}\n";
    tokio::fs::write(&test_file, original_content).await?;

    let mock_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc_edit_2",
                    "type": "function",
                    "function": {
                        "name": "edit_file",
                        "arguments": json!({
                            "path": "hello.rs",
                            "target": "println!(\"Original\");",
                            "replacement": "println!(\"Mutated\");"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
    });

    let final_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "User declined the change."
            }
        }],
        "usage": { "prompt_tokens": 60, "completion_tokens": 10 }
    });

    let server = MockModelServer::start(vec![mock_resp, final_resp]).await?;

    let config = AgentConfig {
        workspace_dir: tmp_workspace.clone(),
        model_tier: ModelTier::Standard128k,
        auto_compaction: false,
        session_id: "diff_reject_test".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "test".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(1024),
        },
        max_tool_iterations: 5,
        mcp_config: None,
        auto_approve_risky: false,
        max_turn_tokens: None,
    };

    let agent = AgendexAgent::init(config)
        .await?
        .with_diff_approval_handler(Arc::new(|_path, _diff| {
            false // Reject edit [N]
        }));

    let result = agent.process_turn("Mutate the greeting").await?;
    assert_eq!(result, "User declined the change.");

    // Verify file on disk was NOT mutated
    let disk_content = tokio::fs::read_to_string(&test_file).await?;
    assert_eq!(disk_content, original_content);

    let _ = tokio::fs::remove_dir_all(&tmp_workspace).await;
    Ok(())
}

#[tokio::test]
async fn test_write_file_existing_diff_rejection() -> anyhow::Result<()> {
    let tmp_workspace =
        std::env::temp_dir().join(format!("agendex_write_diff_rej_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_workspace).await?;
    let test_file = tmp_workspace.join("data.txt");
    let original_content = "initial data\n";
    tokio::fs::write(&test_file, original_content).await?;

    let mock_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "tc_write_1",
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "arguments": json!({
                            "path": "data.txt",
                            "content": "completely replaced data\n",
                            "overwrite": true
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
    });

    let final_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Overwrite was rejected."
            }
        }],
        "usage": { "prompt_tokens": 60, "completion_tokens": 10 }
    });

    let server = MockModelServer::start(vec![mock_resp, final_resp]).await?;

    let config = AgentConfig {
        workspace_dir: tmp_workspace.clone(),
        model_tier: ModelTier::Standard128k,
        auto_compaction: false,
        session_id: "write_diff_reject_test".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "test".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(1024),
        },
        max_tool_iterations: 5,
        mcp_config: None,
        auto_approve_risky: false,
        max_turn_tokens: None,
    };

    let agent = AgendexAgent::init(config)
        .await?
        .with_diff_approval_handler(Arc::new(|_path, _diff| {
            false // Reject overwrite [N]
        }));

    let result = agent.process_turn("Overwrite data.txt").await?;
    assert_eq!(result, "Overwrite was rejected.");

    // File must remain unchanged
    let disk_content = tokio::fs::read_to_string(&test_file).await?;
    assert_eq!(disk_content, original_content);

    let _ = tokio::fs::remove_dir_all(&tmp_workspace).await;
    Ok(())
}

// ============================================================================
// 3. Streaming Chunks & Live Tool Progress Tests
// ============================================================================

#[tokio::test]
async fn test_streaming_tokens_and_tool_progress_callbacks() -> anyhow::Result<()> {
    let tmp_workspace =
        std::env::temp_dir().join(format!("agendex_stream_prog_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&tmp_workspace).await?;

    let tool_call_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I will create a file.",
                "tool_calls": [{
                    "id": "tc_write_file",
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "arguments": json!({
                            "path": "test.txt",
                            "content": "hello streaming\n"
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 40, "completion_tokens": 20 }
    });

    let final_resp = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "File created successfully."
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 10 }
    });

    let server = MockModelServer::start(vec![tool_call_resp, final_resp]).await?;

    let config = AgentConfig {
        workspace_dir: tmp_workspace.clone(),
        model_tier: ModelTier::Standard128k,
        auto_compaction: false,
        session_id: "stream_test".to_string(),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "test".to_string(),
            base_url: Some(server.base_url()),
            temperature: Some(0.0),
            max_output_tokens: Some(1024),
        },
        max_tool_iterations: 5,
        mcp_config: None,
        auto_approve_risky: true,
        max_turn_tokens: None,
    };

    let streamed_tokens = Arc::new(std::sync::Mutex::new(Vec::new()));
    let tokens_clone = Arc::clone(&streamed_tokens);

    let tool_progress_messages = Arc::new(std::sync::Mutex::new(Vec::new()));
    let progress_clone = Arc::clone(&tool_progress_messages);

    let agent = AgendexAgent::init(config)
        .await?
        .with_token_stream_handler(Arc::new(move |token| {
            tokens_clone.lock().unwrap().push(token.to_string());
        }))
        .with_tool_progress_handler(Arc::new(move |msg| {
            progress_clone.lock().unwrap().push(msg.to_string());
        }));

    let reply = agent.process_turn("Create test.txt").await?;
    assert_eq!(reply, "File created successfully.");

    // Verify token streaming was triggered
    let tokens = streamed_tokens.lock().unwrap().clone();
    assert!(
        !tokens.is_empty(),
        "Tokens should be emitted via streaming handler"
    );
    let combined_tokens = tokens.join("");
    assert!(
        combined_tokens.contains("File created successfully.")
            || combined_tokens.contains("I will create a file.")
    );

    // Verify tool progress messages were emitted
    let progress = tool_progress_messages.lock().unwrap().clone();
    assert!(
        !progress.is_empty(),
        "Tool progress messages must be emitted"
    );
    assert!(
        progress
            .iter()
            .any(|m| m.contains("[Tool: write_file] Writing 'test.txt'")),
        "Expected start progress message for write_file, got: {:?}",
        progress
    );
    assert!(
        progress
            .iter()
            .any(|m| m.contains("[Tool: write_file] Completed.")),
        "Expected completion progress message for write_file, got: {:?}",
        progress
    );

    let _ = tokio::fs::remove_dir_all(&tmp_workspace).await;
    Ok(())
}
