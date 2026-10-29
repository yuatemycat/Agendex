mod support;

use agendex_core::{
    AgentConfig, MailboxManager, ModelConfig, ModelFormat, SubagentManager, SubagentPersona,
    WorkspaceMode, WorktreeManager,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_subagent_mailbox_reactive_comm() -> anyhow::Result<()> {
    // A 1-second overall timeout fails the test if it hangs or attempts polling sleep
    tokio::time::timeout(Duration::from_secs(1), async {
        let test_root =
            std::env::temp_dir().join(format!("agendex_test_w1_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&test_root).await?;

        // 1. Script mock model responses:
        // Turn 1: tool call to send_message
        // Turn 2: final textual completion
        let resp_step1 = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "tc_mailbox_1",
                        "type": "function",
                        "function": {
                            "name": "send_message",
                            "arguments": serde_json::json!({
                                "recipient": "parent",
                                "message": "ping from child subagent"
                            }).to_string()
                        }
                    }]
                }
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 18 }
        });

        let resp_step2 = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Child subagent finished execution successfully."
                }
            }],
            "usage": { "prompt_tokens": 30, "completion_tokens": 14 }
        });

        let server = MockModelServer::start(vec![resp_step1, resp_step2]).await?;

        // 2. Configure mailbox, worktree, and subagent manager
        let mailbox = Arc::new(MailboxManager::new());
        let worktree_mgr = Arc::new(WorktreeManager::new(test_root.clone(), None)?);

        let base_config = AgentConfig {
            workspace_dir: test_root.clone(),
            session_id: "parent".to_string(),
            model: ModelConfig {
                format: ModelFormat::OpenAi,
                model_name: "gpt-4o".to_string(),
                api_key: "test_key".to_string(),
                base_url: Some(server.base_url()),
                temperature: Some(0.0),
                max_output_tokens: Some(1024),
            },
            ..Default::default()
        };

        let subagent_mgr = Arc::new(SubagentManager::new(
            "parent",
            base_config,
            Arc::clone(&mailbox),
            Arc::clone(&worktree_mgr),
        ));

        let persona = SubagentPersona::coder();

        let start_time = Instant::now();

        // 3. Parent invokes child subagent
        let child_id = subagent_mgr
            .spawn_custom_subagent(
                persona,
                "Perform background task and report to parent",
                WorkspaceMode::Branch,
            )
            .await?;

        // 4. Parent reactively awaits mailbox message without busy sleep polling
        let incoming_msg = mailbox
            .wait_for_message("parent", Some(Duration::from_millis(800)))
            .await?;

        assert_eq!(incoming_msg.payload, "ping from child subagent");
        assert_eq!(incoming_msg.sender_id, child_id);

        // 5. Parent reactively awaits child completion
        let child_inst = subagent_mgr
            .wait_for_subagent(&child_id, Some(Duration::from_millis(800)))
            .await?;

        assert_eq!(
            child_inst.result.as_deref(),
            Some("Child subagent finished execution successfully.")
        );

        let elapsed = start_time.elapsed();
        assert!(
            elapsed < Duration::from_millis(900),
            "Test took too long: {:?}",
            elapsed
        );

        let _ = tokio::fs::remove_dir_all(&test_root).await;
        Ok::<(), anyhow::Error>(())
    })
    .await?
}
