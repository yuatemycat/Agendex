use agendex_cli::editor::{
    create_default_keybindings, create_reedline_editor, expand_file_mentions,
    sanitize_history_line, SlashCommandInfo,
};
use reedline::{KeyCode, KeyModifiers};

#[test]
fn test_repl_editor_keybindings_and_creation() {
    let tmp = std::env::temp_dir().join(format!("agendex_repl_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();

    let commands = vec![
        SlashCommandInfo {
            name: "/help".into(),
            description: "Show help".into(),
        },
        SlashCommandInfo {
            name: "/model".into(),
            description: "Select model".into(),
        },
    ];

    let editor = create_reedline_editor(&tmp, commands);
    assert!(
        editor.is_ok(),
        "Reedline editor must initialize successfully"
    );

    let bindings = create_default_keybindings();
    // Validate default emacs bindings are active
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Enter)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Backspace)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Left)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Right)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Up)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Down)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::CONTROL, KeyCode::Char('c'))
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::CONTROL, KeyCode::Char('d'))
        .is_some());

    // Validate Agendex custom additions
    assert!(bindings
        .find_binding(KeyModifiers::NONE, KeyCode::Tab)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::SHIFT, KeyCode::BackTab)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::ALT, KeyCode::Enter)
        .is_some());
    assert!(bindings
        .find_binding(KeyModifiers::CONTROL, KeyCode::Char('r'))
        .is_some());

    // Validate '/' and '@' autocomplete triggers
    assert!(
        bindings
            .find_binding(KeyModifiers::NONE, KeyCode::Char('/'))
            .is_some(),
        "Typing '/' must activate autocomplete menu"
    );
    assert!(
        bindings
            .find_binding(KeyModifiers::NONE, KeyCode::Char('@'))
            .is_some(),
        "Typing '@' must activate file autocomplete menu"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_sanitized_history_prevents_api_key_leakage() {
    // 7-part syntax
    let input =
        "/provider add my-vllm http://127.0.0.1:8000/v1 custom-model openai sk-live-secret-987654";
    let scrubbed = sanitize_history_line(input);
    assert!(!scrubbed.contains("sk-live-secret-987654"));
    assert!(scrubbed.contains("[REDACTED]"));

    // 5-part syntax (/provider add <name> <url> <key>)
    let input5 = "/provider add my-nim https://integrate.api.nvidia.com nvapi-supersecret-99999";
    let scrubbed5 = sanitize_history_line(input5);
    assert!(!scrubbed5.contains("nvapi-supersecret-99999"));
    assert!(scrubbed5.contains("[REDACTED]"));

    // .env pasted into prompt
    let env_line = "OPENAI_API_KEY=sk-test-live-key-1234567890123456";
    let scrubbed_env = sanitize_history_line(env_line);
    assert!(!scrubbed_env.contains("sk-test-live-key-1234567890123456"));
    assert!(scrubbed_env.contains("[REDACTED]"));

    // Environment variable references must remain intact
    let env_input = "/provider add my-vllm http://127.0.0.1:8000/v1 custom-model openai $VLLM_KEY";
    let env_scrubbed = sanitize_history_line(env_input);
    assert_eq!(env_scrubbed, env_input);
}

#[tokio::test]
async fn test_revert_skips_turns_without_file_changes() -> anyhow::Result<()> {
    let tmp =
        std::env::temp_dir().join(format!("agendex_revert_skip_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp)?;

    let mut engine = agendex_trajectory::SnapshotEngine::new(&tmp)?;

    // Baseline snapshot
    let _base = engine.capture_snapshot("baseline").await?;

    // Turn 1: modifies a file
    let file_a = tmp.join("file_a.txt");
    std::fs::write(&file_a, "initial content")?;
    engine
        .capture_snapshot_with_metadata(
            "turn_start:1",
            Some(serde_json::json!({
                "kind": "turn_start",
                "step_index": 1,
                "created_files": ["file_a.txt"]
            })),
        )
        .await?;

    // Agent modifies file_a in Turn 1
    std::fs::write(&file_a, "modified content by agent")?;

    // Turn 2: user asks a question, agent changes NO files
    engine
        .capture_snapshot_with_metadata(
            "turn_start:2",
            Some(serde_json::json!({
                "kind": "turn_start",
                "step_index": 2,
                "created_files": []
            })),
        )
        .await?;

    // Workspace is still in the same state as end of Turn 1.
    // Now user calls rollback_turn()! It should skip Turn 2 and revert Turn 1's changes!
    let res = engine.rollback_turn().await?;
    assert!(res.is_some());
    let _roll = res.unwrap();
    // It should have restored to turn_start:1 or baseline, removing the agent-created file
    assert!(!file_a.exists() || std::fs::read_to_string(&file_a)? == "initial content");

    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

#[tokio::test]
async fn test_resume_rejects_path_traversal() -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join(format!("agendex_resume_test_{}", uuid::Uuid::new_v4()));
    let ws = tmp.join("ws");
    std::fs::create_dir_all(&ws)?;

    let config = agendex_core::agent::AgentConfig {
        workspace_dir: ws.clone(),
        session_id: format!("test_session_{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };

    let agent = agendex_core::agent::AgendexAgent::init(config).await?;

    // Attempt traversal with ..
    let res = agent.handle_resume_command("../outside_session").await?;
    assert!(
        res.contains("prohibited") || res.contains("traversal"),
        "Should reject traversal in /resume"
    );

    // Attempt traversal with slashes
    let res_slash = agent.handle_resume_command("dir/sub").await?;
    assert!(
        res_slash.contains("prohibited") || res_slash.contains("traversal"),
        "Should reject slashes in /resume"
    );

    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

#[test]
fn test_at_file_workspace_isolation_and_large_multibyte() {
    let tmp = std::env::temp_dir().join(format!("agendex_atfile_test_{}", uuid::Uuid::new_v4()));
    let ws = tmp.join("ws");
    let outside = tmp.join("outside");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&outside).unwrap();

    let secret = outside.join("secret.txt");
    std::fs::write(&secret, "SUPER_SECRET_TOKEN").unwrap();

    // 1. Traversal outside workspace
    let res = expand_file_mentions(&ws, "Here is @../../outside/secret.txt");
    assert!(!res.contains("SUPER_SECRET_TOKEN"));

    // 2. Large multibyte file boundary safety
    let mut large = String::new();
    for _ in 0..5000 {
        large.push_str("🚀 Rust🦀安全");
    }
    let large_file = ws.join("large_multibyte.txt");
    std::fs::write(&large_file, &large).unwrap();

    let expanded = expand_file_mentions(&ws, "Look at @large_multibyte.txt");
    assert!(expanded.contains("[Truncated - file exceeds 30KB]"));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn test_repl_piped_interactive_execution_isolated_home() -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join(format!("agendex_term_test_{}", uuid::Uuid::new_v4()));
    let home = tmp.join("home");
    let ws = tmp.join("ws");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&ws)?;

    // Set AGENDEX_HOME to ensure zero interaction with real user directory
    std::env::set_var("AGENDEX_HOME", &home);

    let config = agendex_core::agent::AgentConfig {
        workspace_dir: ws.clone(),
        session_id: format!("term_session_{}", uuid::Uuid::new_v4()),
        ..Default::default()
    };

    let agent = agendex_core::agent::AgendexAgent::init(config).await?;

    // Execute slash commands
    let help_res = agent.process_turn("/help").await?;
    assert!(help_res.contains("✦ Agendex Command & Skill Switchboard ✦"));

    let context_res = agent.process_turn("/context").await?;
    assert!(context_res.contains("Context Window Gauge"));

    let cost_res = agent.process_turn("/cost").await?;
    assert!(cost_res.contains("Session Cost Breakdown"));

    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
