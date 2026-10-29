use agendex_cli::{parse_cli_args, try_parse_cli_args};
use agendex_core::model::ModelFormat;
use agendex_trajectory::compactor::ModelTier;
use std::path::PathBuf;

#[test]
fn test_cli_flags_parsing() {
    let args = vec![
        "agendex".to_string(),
        "--format".to_string(),
        "anthropic".to_string(),
        "--model".to_string(),
        "claude-3-5-sonnet-20241022".to_string(),
        "--base-url".to_string(),
        "http://127.0.0.1:9090".to_string(),
        "--api-key".to_string(),
        "sk-ant-test".to_string(),
        "--tier".to_string(),
        "128k".to_string(),
        "--mcp-config".to_string(),
        "mcp_custom.json".to_string(),
        "--auto-approve".to_string(),
        "-p".to_string(),
        "audit and test the codebase".to_string(),
    ];

    let parsed = parse_cli_args(&args, PathBuf::from("/workspace"));

    assert_eq!(
        parsed.prompt.as_deref(),
        Some("audit and test the codebase")
    );
    assert_eq!(parsed.config.model.format, ModelFormat::Anthropic);
    assert_eq!(parsed.config.model.model_name, "claude-3-5-sonnet-20241022");
    assert_eq!(
        parsed.config.model.base_url.as_deref(),
        Some("http://127.0.0.1:9090")
    );
    assert_eq!(parsed.config.model.api_key, "sk-ant-test");
    assert_eq!(parsed.config.model_tier, ModelTier::Standard128k);
    assert_eq!(
        parsed.config.mcp_config,
        Some(PathBuf::from("mcp_custom.json"))
    );
    assert!(parsed.config.auto_approve_risky);
    assert!(!parsed.show_help);
    assert!(!parsed.json_mode);
    assert!(!parsed.git_commit);
    assert!(parsed.git_branch.is_none());
}

#[test]
fn test_cli_advanced_tooling_flags() {
    let args = vec![
        "agendex".to_string(),
        "--json".to_string(),
        "--git-commit".to_string(),
        "--git-branch".to_string(),
        "feat/tooling".to_string(),
        "-p".to_string(),
        "run headless".to_string(),
    ];

    let parsed = parse_cli_args(&args, PathBuf::from("."));

    assert!(parsed.json_mode);
    assert!(parsed.git_commit);
    assert_eq!(parsed.git_branch.as_deref(), Some("feat/tooling"));
    assert_eq!(parsed.prompt.as_deref(), Some("run headless"));
}

#[test]
fn test_cli_config_file_and_proxymateur_preset() {
    let tmp_dir = std::env::temp_dir().join(format!("agendex_cli_cfg_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let cfg_file = tmp_dir.join("config.toml");
    std::fs::write(
        &cfg_file,
        r#"
            provider = "proxymateur"
            model = "gpt-4o"
            tier = "256k"
            auto_approve_risky = true
        "#,
    )
    .unwrap();

    let args = vec![
        "agendex".to_string(),
        "--config".to_string(),
        cfg_file.to_string_lossy().to_string(),
    ];

    let parsed = parse_cli_args(&args, PathBuf::from("."));

    assert_eq!(parsed.config.model.format, ModelFormat::OpenAi);
    assert_eq!(
        parsed.config.model.base_url.as_deref(),
        Some("http://127.0.0.1:8045/v1")
    );
    assert_eq!(parsed.config.model.model_name, "gpt-4o");
    assert_eq!(parsed.config.model_tier, ModelTier::HighTier256k);
    assert!(parsed.config.auto_approve_risky);

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_cli_provider_proxymateur_flag() {
    let args = vec![
        "agendex".to_string(),
        "--provider".to_string(),
        "proxymateur".to_string(),
        "--model".to_string(),
        "gemini-2.5-flash".to_string(),
    ];

    let parsed = parse_cli_args(&args, PathBuf::from("."));

    assert_eq!(parsed.config.model.format, ModelFormat::OpenAi);
    assert_eq!(
        parsed.config.model.base_url.as_deref(),
        Some("http://127.0.0.1:8045/v1")
    );
    assert_eq!(parsed.config.model.model_name, "gemini-2.5-flash");
}

#[test]
fn test_bad_config_file_returns_error() {
    let args = vec![
        "agendex".to_string(),
        "--config".to_string(),
        "non_existent_config_file_12345.toml".to_string(),
    ];

    let res = try_parse_cli_args(&args, PathBuf::from("."));
    assert!(res.is_err(), "Expected error on bad --config file path");
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Failed to load configuration file"),
        "Unexpected error: {}",
        err_str
    );
}

#[test]
fn test_cli_provider_nim_flag() {
    let args = vec![
        "agendex".to_string(),
        "--provider".to_string(),
        "nim".to_string(),
    ];

    let parsed = parse_cli_args(&args, PathBuf::from("."));

    assert_eq!(parsed.config.model.format, ModelFormat::OpenAi);
    assert_eq!(
        parsed.config.model.base_url.as_deref(),
        Some("https://integrate.api.nvidia.com/v1")
    );
    assert_eq!(
        parsed.config.model.model_name,
        "meta/llama-3.1-70b-instruct"
    );
}

#[test]
fn test_cli_provider_strata_flag() {
    let args = vec![
        "agendex".to_string(),
        "--provider".to_string(),
        "strata".to_string(),
    ];

    let parsed = parse_cli_args(&args, PathBuf::from("."));

    assert_eq!(parsed.config.model.format, ModelFormat::OpenAi);
    assert_eq!(
        parsed.config.model.base_url.as_deref(),
        Some("http://127.0.0.1:8080/v1")
    );
    assert_eq!(parsed.config.model.model_name, "huihui-swift-1.5");
    assert_eq!(parsed.config.model.api_key, "strata");
}
