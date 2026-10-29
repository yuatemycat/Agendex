mod support;

use agendex_core::{AgendexAgent, AgentConfig, ModelConfig, ModelFormat};
use serde_json::json;
use std::sync::Arc;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_token_count_and_cost_accounting_lifecycle() -> anyhow::Result<()> {
    let test_dir =
        std::env::temp_dir().join(format!("agendex_telemetry_test_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    let session_id = format!("telemetry_session_{}", uuid::Uuid::new_v4());

    // Turn 1 mock response: 412 prompt tokens, 85 completion tokens
    let resp1 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Turn 1 complete."
            }
        }],
        "usage": { "prompt_tokens": 412, "completion_tokens": 85 }
    });

    // Turn 2 mock response: 500 prompt tokens, 100 completion tokens
    let resp2 = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Turn 2 complete."
            }
        }],
        "usage": { "prompt_tokens": 500, "completion_tokens": 100 }
    });

    let server = MockModelServer::start(vec![resp1, resp2]).await?;

    let agent = AgendexAgent::init(AgentConfig {
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
    })
    .await?;

    // Execute Turn 1
    let reply1 = agent.process_turn("Run step 1").await?;
    assert!(reply1.contains("Turn 1 complete"));

    let turn1_usage = agent.last_turn_usage().await.expect("Turn 1 usage missing");
    assert_eq!(turn1_usage.prompt_tokens, 412);
    assert_eq!(turn1_usage.completion_tokens, 85);
    assert_eq!(turn1_usage.total_tokens, 497);
    // Cost: 412 * 0.0000025 + 85 * 0.000010 = 0.00188
    assert!((turn1_usage.estimated_cost - 0.00188).abs() < 1e-5);

    let session1_usage = agent.session_usage().await;
    assert_eq!(session1_usage.total_prompt_tokens, 412);
    assert_eq!(session1_usage.total_completion_tokens, 85);
    assert_eq!(session1_usage.total_tokens, 497);
    assert_eq!(session1_usage.turn_count, 1);

    let telemetry_str1 = agent
        .format_telemetry()
        .await
        .expect("Telemetry string missing");
    assert!(
        telemetry_str1.contains("[Tokens: 412 in, 85 out | Session: 497 ($0.0019)]"),
        "Telemetry display was: {}",
        telemetry_str1
    );

    // Execute Turn 2
    let reply2 = agent.process_turn("Run step 2").await?;
    assert!(reply2.contains("Turn 2 complete"));

    let turn2_usage = agent.last_turn_usage().await.expect("Turn 2 usage missing");
    assert_eq!(turn2_usage.prompt_tokens, 500);
    assert_eq!(turn2_usage.completion_tokens, 100);

    let session2_usage = agent.session_usage().await;
    assert_eq!(session2_usage.total_prompt_tokens, 912);
    assert_eq!(session2_usage.total_completion_tokens, 185);
    assert_eq!(session2_usage.total_tokens, 1097);
    assert_eq!(session2_usage.turn_count, 2);

    // Resume the agent in the same session and verify cumulative session usage is restored from SQLite
    let resumed_agent = AgendexAgent::init(AgentConfig {
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
    })
    .await?;

    let restored_session_usage = resumed_agent.session_usage().await;
    assert_eq!(restored_session_usage.total_prompt_tokens, 912);
    assert_eq!(restored_session_usage.total_completion_tokens, 185);
    assert_eq!(restored_session_usage.total_tokens, 1097);
    assert_eq!(restored_session_usage.turn_count, 2);

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}

#[tokio::test]
async fn test_cross_session_history_search() -> anyhow::Result<()> {
    let test_dir =
        std::env::temp_dir().join(format!("agendex_search_test_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    let session_1_id = format!("session_alpha_{}", uuid::Uuid::new_v4());
    let session_2_id = format!("session_beta_{}", uuid::Uuid::new_v4());

    let resp_alpha = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Configured PostgreSQL high-availability replication cluster."
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
    });

    let resp_beta = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "Configured Redis cache cluster."
            }
        }],
        "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
    });

    let server = MockModelServer::start(vec![resp_alpha, resp_beta]).await?;

    // Session 1: Mentions PostgreSQL
    let agent_1 = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: session_1_id.clone(),
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

    agent_1.process_turn("Setup PostgreSQL cluster").await?;

    // Session 2: Mentions Redis
    let agent_2 = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: session_2_id.clone(),
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

    agent_2.process_turn("Setup Redis cluster").await?;

    // Now, perform cross-session trajectory search using the tool registry
    let search_res = agent_2
        .tool_registry
        .execute_tool(
            "search_history",
            json!({
                "query": "PostgreSQL"
            }),
        )
        .await?;

    let results: Vec<agendex_trajectory::HistorySearchResult> = serde_json::from_value(search_res)?;
    assert!(
        !results.is_empty(),
        "Expected to find past PostgreSQL steps"
    );
    assert_eq!(results[0].session_id, session_1_id);
    assert!(results[0].snippet.contains("PostgreSQL"));

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}

#[tokio::test]
async fn test_save_skill_approval_gate_and_immediate_slash_command_dispatch() -> anyhow::Result<()>
{
    let test_dir =
        std::env::temp_dir().join(format!("agendex_skill_gate_{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&test_dir).await?;

    // Turn 1: Model calls save_skill tool
    let resp_save_call = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_save_1",
                    "type": "function",
                    "function": {
                        "name": "save_skill",
                        "arguments": serde_json::to_string(&json!({
                            "name": "canary-diagnostic",
                            "description": "Runs canary performance audit across nodes",
                            "content": "# /canary-diagnostic Protocol\nExecute canary probes and report latency."
                        }))?
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 60, "completion_tokens": 30 }
    });

    let resp_final = json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "I have saved the canary-diagnostic skill successfully."
            }
        }],
        "usage": { "prompt_tokens": 100, "completion_tokens": 15 }
    });

    let server = MockModelServer::start(vec![resp_save_call.clone(), resp_final.clone()]).await?;

    // 1. Rejection test: approval handler returns false
    let agent_denied = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: format!("session_denied_{}", uuid::Uuid::new_v4()),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "dummy".to_string(),
            base_url: Some(server.base_url()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?
    .with_approval_handler(Arc::new(|_action, _reason| false));

    let _ = agent_denied.process_turn("Save canary skill").await;
    let expected_skill_file = test_dir
        .join(".agenda")
        .join("skills")
        .join("canary-diagnostic")
        .join("SKILL.md");
    assert!(
        !expected_skill_file.exists(),
        "Skill file must not exist when approval is denied"
    );

    // 2. Approval test: approval handler returns true
    let server_approved = MockModelServer::start(vec![resp_save_call, resp_final]).await?;
    let agent_approved = AgendexAgent::init(AgentConfig {
        workspace_dir: test_dir.clone(),
        session_id: format!("session_approved_{}", uuid::Uuid::new_v4()),
        model: ModelConfig {
            format: ModelFormat::OpenAi,
            model_name: "gpt-4o".to_string(),
            api_key: "dummy".to_string(),
            base_url: Some(server_approved.base_url()),
            ..Default::default()
        },
        ..Default::default()
    })
    .await?
    .with_approval_handler(Arc::new(|_action, _reason| true));

    let reply = agent_approved.process_turn("Save canary skill").await?;
    assert!(reply.contains("saved the canary-diagnostic skill"));
    assert!(
        expected_skill_file.exists(),
        "Skill file must exist on approval"
    );

    let saved_content = tokio::fs::read_to_string(&expected_skill_file).await?;
    assert!(saved_content.contains("name: canary-diagnostic"));
    assert!(saved_content.contains("Runs canary performance audit"));

    // 3. Verify the newly saved skill is immediately callable via slash command
    let slash_reply = agent_approved.process_turn("/canary-diagnostic").await?;
    assert!(slash_reply.contains("Skill [canary-diagnostic]"));
    assert!(slash_reply.contains("Runs canary performance audit"));
    assert!(slash_reply.contains("Execute canary probes and report latency"));

    let _ = tokio::fs::remove_dir_all(&test_dir).await;
    Ok(())
}
