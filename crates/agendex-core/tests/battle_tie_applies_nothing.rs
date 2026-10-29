mod support;

use agendex_core::{
    AgentConfig, BattleArena, BattleDuelConfig, BattleVerdict, MailboxManager, ModelConfig,
    ModelFormat, SubagentManager, SubagentPersona, WorkspaceMode, WorktreeManager,
};
use std::sync::Arc;
use support::mock_model::MockModelServer;

#[tokio::test]
async fn test_battle_tie_applies_nothing() -> anyhow::Result<()> {
    let test_root = std::env::temp_dir().join(format!("agendex_test_w5_{}", uuid::Uuid::new_v4()));
    let base_repo = test_root.join("parent_repo");
    let wt_root = test_root.join("worktrees");
    tokio::fs::create_dir_all(base_repo.join("src")).await?;

    // Seed original file
    let calc_file = base_repo.join("src").join("calc.rs");
    let original_bytes = "pub fn add(a: i32, b: i32) -> i32 { 0 }\n";
    tokio::fs::write(&calc_file, original_bytes).await?;

    // Verification script requiring 'a + b'
    let verify_script = base_repo.join("verify.py");
    tokio::fs::write(
        &verify_script,
        "with open('src/calc.rs') as f:\n    assert 'a + b' in f.read()\n",
    )
    .await?;

    // Script mock responses: both children write code that fails verification
    let a_turn1 = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "tc_a_1",
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "arguments": serde_json::json!({
                            "path": "src/calc.rs",
                            "content": "pub fn add(a: i32, b: i32) -> i32 { 10 }\n",
                            "overwrite": true
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 10 }
    });
    let a_turn2 = serde_json::json!({
        "choices": [{
            "message": { "role": "assistant", "content": "Child A finished." }
        }],
        "usage": { "prompt_tokens": 20, "completion_tokens": 5 }
    });

    let b_turn1 = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "tc_b_1",
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "arguments": serde_json::json!({
                            "path": "src/calc.rs",
                            "content": "pub fn add(a: i32, b: i32) -> i32 { 20 }\n",
                            "overwrite": true
                        }).to_string()
                    }
                }]
            }
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 10 }
    });
    let b_turn2 = serde_json::json!({
        "choices": [{
            "message": { "role": "assistant", "content": "Child B finished." }
        }],
        "usage": { "prompt_tokens": 20, "completion_tokens": 5 }
    });

    let server = MockModelServer::start(vec![a_turn1, a_turn2, b_turn1, b_turn2]).await?;

    let mailbox = Arc::new(MailboxManager::new());
    let worktree_mgr = Arc::new(WorktreeManager::new(base_repo.clone(), Some(wt_root))?);

    let base_config = AgentConfig {
        workspace_dir: base_repo.clone(),
        session_id: "arena_parent".to_string(),
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
        "arena_parent",
        base_config,
        Arc::clone(&mailbox),
        Arc::clone(&worktree_mgr),
    ));

    let arena = BattleArena::new(subagent_mgr, Arc::clone(&worktree_mgr));

    let mut persona_a = SubagentPersona::coder();
    persona_a.name = "combatant_alpha".to_string();

    let mut persona_b = SubagentPersona::coder();
    persona_b.name = "combatant_beta".to_string();

    // Find available python runtime: python3 or python
    let python_bin = if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "python3"
    } else {
        "python"
    };

    let duel_config = BattleDuelConfig {
        task_prompt: "Implement add in src/calc.rs".to_string(),
        combatant_a: persona_a,
        combatant_b: persona_b,
        verification_cmd: Some(format!("{} verify.py", python_bin)),
        workspace_mode: WorkspaceMode::Share,
        apply_winner: true,
        cleanup_worktrees: true,
        timeout: Some(std::time::Duration::from_secs(30)),
    };

    let report = arena.duel(duel_config).await?;

    // Both combatants failed verification
    assert!(!report.combatant_a.verification_passed);
    assert!(!report.combatant_b.verification_passed);

    // Verdict must be NoWinner
    assert_eq!(report.verdict, BattleVerdict::NoWinner);
    assert!(!report.applied_to_parent);

    // The parent file remains completely unchanged
    let current_parent_content = tokio::fs::read_to_string(&calc_file).await?;
    assert_eq!(current_parent_content, original_bytes);

    let _ = tokio::fs::remove_dir_all(&test_root).await;
    Ok(())
}
