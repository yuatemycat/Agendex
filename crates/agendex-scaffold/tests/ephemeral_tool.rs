use agendex_scaffold::{EphemeralToolRegistry, ToolRuntime};
use std::env;
use std::path::PathBuf;

fn temp_test_dir(prefix: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = env::temp_dir().join(format!(
        "agendex_test_{}_{}_{}",
        prefix,
        std::process::id(),
        ts
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[tokio::test]
async fn test_ephemeral_tool_synthesis_and_validation_filtering() {
    let temp_dir = temp_test_dir("s5_ephemeral");
    let registry = EphemeralToolRegistry::new(&temp_dir)
        .await
        .expect("Failed to create ephemeral tool registry");

    // 1. A Python script that prints {"ok":true} is synthesized and appears in the tool list
    let script1_code = "print('{\"ok\":true}')";
    let res1 = registry
        .synthesize_tool(
            "valid_probe",
            "A valid ephemeral Python diagnostic probe",
            ToolRuntime::Python,
            script1_code,
            None,
        )
        .await;

    assert!(
        res1.is_ok(),
        "Synthesizing valid tool should succeed: {:?}",
        res1.err()
    );
    let tool1 = res1.unwrap();
    assert_eq!(tool1.name, "valid_probe");

    let tools_after_first = registry.list_tools().await;
    assert_eq!(tools_after_first.len(), 1);
    assert_eq!(tools_after_first[0].name, "valid_probe");
    assert!(registry.has_tool("valid_probe").await);

    // 2. A second script that exits 1 does not appear in the tool list
    let script2_code = r#"
import sys
sys.stderr.write("Fatal script error\n")
sys.exit(1)
"#;
    let res2 = registry
        .synthesize_tool(
            "faulty_probe",
            "A failing ephemeral probe",
            ToolRuntime::Python,
            script2_code,
            None,
        )
        .await;

    assert!(
        res2.is_err(),
        "Synthesizing failing tool should return an error"
    );

    let tools_after_second = registry.list_tools().await;
    assert_eq!(
        tools_after_second.len(),
        1,
        "Tool list should still only contain the valid tool"
    );
    assert_eq!(tools_after_second[0].name, "valid_probe");
    assert!(!registry.has_tool("faulty_probe").await);

    // Verify the faulty script was not left in scratch directory
    let faulty_path = registry.scratch_dir().join("faulty_probe.py");
    assert!(
        !faulty_path.exists(),
        "Failed tool script must be cleaned up from scratch dir"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}
