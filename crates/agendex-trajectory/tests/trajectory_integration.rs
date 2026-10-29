use agendex_trajectory::{
    CompactorEngine, ModelTier, SnapshotEngine, TrajectoryLogger, STRATEGY_ANCHORED_HEAD_TAIL,
};
use std::fs::File;
use std::io::{BufRead, BufReader};

#[tokio::test]
async fn test_trajectory_full_lifecycle() -> anyhow::Result<()> {
    let test_root = std::env::temp_dir().join(format!("agendex_traj_e2e_{}", uuid::Uuid::new_v4()));
    let workspace_dir = test_root.join("workspace");
    let log_dir = test_root.join("logs");

    tokio::fs::create_dir_all(&workspace_dir).await?;
    tokio::fs::create_dir_all(&log_dir).await?;

    // 1. Initialize SnapshotEngine, TrajectoryLogger, and CompactorEngine
    let mut snapshot_engine = SnapshotEngine::new(&workspace_dir)?;
    let mut trajectory = TrajectoryLogger::new(&log_dir)?;
    let mut compactor = CompactorEngine::new(ModelTier::Compact80k);

    // Initial project file
    let main_rs = workspace_dir.join("main.rs");
    tokio::fs::write(&main_rs, "fn main() { println!(\"Hello World\"); }\n").await?;

    // Step 1: Pre-turn snapshot & user input
    let snap_initial = snapshot_engine.capture_snapshot("turn_1_start").await?;
    let step1 = trajectory
        .log_step(
            "USER_INPUT",
            "USER",
            "Build an AST parser and add error handling to main.rs",
            serde_json::json!({"turn": 1}),
        )
        .await?;
    assert_eq!(step1.step_index, 1);

    // Record checkpoint linking step 1 to snapshot tree hash
    let cp_id = trajectory.save_checkpoint("checkpoint_turn_1", Some(&snap_initial.tree_hash))?;
    assert!(cp_id > 0);

    // Step 2..8: Simulated planner & tool execution turns
    for turn in 2..=8 {
        let content = format!(
            "Execution log for turn {}: analyzing code structures and writing AST visitor.",
            turn
        );
        trajectory
            .log_step(
                "TOOL_RESULT",
                "TOOL",
                &content,
                serde_json::json!({"turn": turn, "tool": "ast_patcher"}),
            )
            .await?;
    }
    assert_eq!(trajectory.step_count(), 8);

    // Step 9: Compaction condition evaluation
    // 80k tier has effective threshold at 68,000 tokens
    assert!(!compactor.should_compact(60_000));
    assert!(compactor.should_compact(72_000));

    // Execute Head-Tail compaction on trajectory
    let all_steps = trajectory.get_steps();
    let compacted = compactor.compact_trajectory(all_steps, 72_000, None);

    assert_eq!(compacted.strategy, STRATEGY_ANCHORED_HEAD_TAIL);
    // Preserves head=2, tail=4, evicts middle 2 steps (8 - 6 = 2)
    assert_eq!(compacted.evicted_count, 2);
    assert_eq!(compacted.retained_steps.len(), 7); // 2 head + 1 compaction anchor + 4 tail

    // Log compaction event into trajectory
    let comp_step = trajectory
        .log_step(
            "COMPACTION",
            "SYSTEM",
            &compacted.continuation_summary,
            serde_json::json!({
                "strategy": STRATEGY_ANCHORED_HEAD_TAIL,
                "evicted_turns": compacted.evicted_count,
            }),
        )
        .await?;
    assert_eq!(comp_step.step_type, "COMPACTION");

    // Post-compaction immunity prevents back-to-back compaction churn
    assert!(compactor.has_immunity());
    assert!(!compactor.should_compact(75_000));

    // Advancing turn clears immunity
    compactor.advance_turn();
    assert!(!compactor.has_immunity());
    assert!(compactor.should_compact(75_000));

    // Step 10: Destructive file mutation and subsequent rollback test
    tokio::fs::write(&main_rs, "MALFORMED BROKEN CODE SYNTAX ERROR\n").await?;
    let temp_untracked = workspace_dir.join("corrupted.tmp");
    tokio::fs::write(&temp_untracked, "stray data").await?;

    // Perform rollback to snap_initial
    let restored_snap = snapshot_engine.rollback("turn_1_start").await?;
    assert_eq!(restored_snap.tree_hash, snap_initial.tree_hash);

    let restored_content = tokio::fs::read_to_string(&main_rs).await?;
    assert_eq!(
        restored_content.replace("\r\n", "\n"),
        "fn main() { println!(\"Hello World\"); }\n"
    );
    assert!(!temp_untracked.exists());

    // Rollback trajectory history to step 1
    trajectory.truncate_after(1)?;
    assert_eq!(trajectory.step_count(), 1);

    // Verify trajectory files truncated accordingly
    let compact_reader = BufReader::new(File::open(log_dir.join("trajectory.jsonl"))?);
    let lines: Vec<String> = compact_reader.lines().collect::<Result<_, _>>()?;
    assert_eq!(lines.len(), 1);

    // Cleanup
    let _ = tokio::fs::remove_dir_all(&test_root).await;
    Ok(())
}
