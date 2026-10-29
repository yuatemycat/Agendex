use agendex_ast::{AstPatcher, PatchError, SyntaxError};
use std::fs;

#[test]
fn test_syntax_gate_rejects_broken_patch_and_preserves_file() {
    let temp_dir = std::env::temp_dir().join(format!("agendex_syntax_gate_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temporary directory");
    let test_file = temp_dir.join("main.rs");

    let initial_content = "fn run() {\n    println!(\"hello world\");\n}\n";
    fs::write(&test_file, initial_content).expect("Failed to write initial source file");

    let patcher = AstPatcher::new();

    // Attempt replacing the valid function statement with an invalid declaration "fn {"
    let patch_result = patcher.patch_file(&test_file, "println!(\"hello world\");", "fn {");

    assert!(
        patch_result.is_err(),
        "Patch must fail syntax validation gate"
    );
    let err = patch_result.unwrap_err();

    match err {
        PatchError::Syntax(SyntaxError::InvalidDeclaration {
            language, message, ..
        }) => {
            assert_eq!(language, agendex_ast::SupportedLanguage::Rust);
            assert!(message.contains("expected identifier after 'fn'"));
        }
        PatchError::Syntax(SyntaxError::UnclosedDelimiter { delimiter, .. }) => {
            assert_eq!(delimiter, '{');
        }
        _ => panic!("Expected PatchError::Syntax variant, got {:?}", err),
    }

    // Verify file on disk is completely untouched byte-for-byte
    let current_content = fs::read_to_string(&test_file).expect("Failed to read file back");
    assert_eq!(
        current_content, initial_content,
        "File on disk must remain unchanged when syntax gate fails"
    );

    let _ = fs::remove_dir_all(temp_dir);
}
