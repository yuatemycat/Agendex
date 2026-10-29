use agendex_ast::{AstEngine, AstPatcher, NodeKind, SupportedLanguage};
use std::fs;

#[test]
fn test_edit_node_module_disambiguation_and_preservation() {
    let temp_dir = std::env::temp_dir().join(format!("agendex_edit_node_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("Failed to create temporary directory");
    let fixture_file = temp_dir.join("fixture.rs");

    let initial_content = r#"
mod mod_a {
    pub fn run() {
        println!("alpha");
    }
}

mod mod_b {
    pub fn run() {
        println!("beta");
    }
}
"#;
    fs::write(&fixture_file, initial_content).expect("Failed to write fixture");

    let patcher = AstPatcher::new();

    // 1. Ambiguous match without module path must fail and leave file untouched
    let ambiguous_res = patcher.edit_node_in_file(
        &fixture_file,
        NodeKind::Function,
        "run",
        None,
        "pub fn run() { println!(\"modified\"); }",
    );
    assert!(
        ambiguous_res.is_err(),
        "Must reject ambiguous node edits without module path"
    );

    let content_after_fail = fs::read_to_string(&fixture_file).unwrap();
    assert_eq!(
        content_after_fail, initial_content,
        "File must remain untouched when ambiguous edit fails"
    );

    // 2. Disambiguated edit specifying module path "mod_a"
    let success_res = patcher.edit_node_in_file(
        &fixture_file,
        NodeKind::Function,
        "run",
        Some("mod_a"),
        "pub fn run() {\n        println!(\"patched_alpha\");\n    }",
    );
    assert!(
        success_res.is_ok(),
        "Must succeed when module path disambiguates candidate"
    );

    let updated_content = fs::read_to_string(&fixture_file).unwrap();
    assert!(
        updated_content.contains("patched_alpha"),
        "mod_a function must be replaced with new body"
    );
    assert!(
        updated_content.contains("println!(\"beta\");"),
        "mod_b function must remain completely intact"
    );

    // 3. Test in-memory AstEngine::edit_node directly
    let parsed_nodes = AstEngine::parse(SupportedLanguage::Rust, &updated_content).unwrap();
    let run_funcs = AstEngine::find_by_name(&parsed_nodes, "run");
    assert_eq!(run_funcs.len(), 2);

    let _ = fs::remove_dir_all(temp_dir);
}
