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

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_sanitized_history_prevents_api_key_leakage() {
    let input =
        "/provider add my-vllm http://127.0.0.1:8000/v1 custom-model openai sk-live-secret-987654";
    let scrubbed = sanitize_history_line(input);
    assert!(!scrubbed.contains("sk-live-secret-987654"));
    assert!(scrubbed.contains("[REDACTED]"));

    let env_input = "/provider add my-vllm http://127.0.0.1:8000/v1 custom-model openai $VLLM_KEY";
    let env_scrubbed = sanitize_history_line(env_input);
    assert_eq!(env_scrubbed, env_input);
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
