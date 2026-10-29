use agendex_cli::parse_cli_args;
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
}
