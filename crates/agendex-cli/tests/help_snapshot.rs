use agendex_cli::parse_cli_args;
use std::path::PathBuf;

#[test]
fn test_help_flag() {
    let args = vec!["agendex".to_string(), "--help".to_string()];
    let parsed = parse_cli_args(&args, PathBuf::from("."));
    assert!(parsed.show_help);

    let short_args = vec!["agendex".to_string(), "-h".to_string()];
    let short_parsed = parse_cli_args(&short_args, PathBuf::from("."));
    assert!(short_parsed.show_help);
}
