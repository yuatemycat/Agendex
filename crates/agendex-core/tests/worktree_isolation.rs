use agendex_core::{WorkspaceMode, WorktreeManager};
use std::path::PathBuf;

#[tokio::test]
async fn test_worktree_share_isolation_and_apply() -> anyhow::Result<()> {
    let test_root = std::env::temp_dir().join(format!("agendex_test_w2_{}", uuid::Uuid::new_v4()));
    let base_repo = test_root.join("parent_repo");
    let wt_root = test_root.join("worktrees");

    tokio::fs::create_dir_all(base_repo.join("src")).await?;

    let parent_file: PathBuf = base_repo.join("src").join("lib.rs");
    tokio::fs::write(&parent_file, "pub fn magic() -> u32 { 100 }\n").await?;

    let worktree_mgr = WorktreeManager::new(base_repo.clone(), Some(wt_root))?;

    // Create two independent Share workspaces backed by git worktree
    let ws1 = worktree_mgr
        .create_workspace("child_agent_1", WorkspaceMode::Share)
        .await?;
    let ws2 = worktree_mgr
        .create_workspace("child_agent_2", WorkspaceMode::Share)
        .await?;

    let file_ws1 = ws1.path.join("src").join("lib.rs");
    let file_ws2 = ws2.path.join("src").join("lib.rs");

    assert!(file_ws1.exists());
    assert!(file_ws2.exists());

    // Both children write different bytes to the exact same relative path
    tokio::fs::write(&file_ws1, "pub fn magic() -> u32 { 200 }\n").await?;
    tokio::fs::write(&file_ws2, "pub fn magic() -> u32 { 300 }\n").await?;

    // Verify isolation: Each worktree maintains its own independent bytes
    let content_ws1 = tokio::fs::read_to_string(&file_ws1).await?;
    let content_ws2 = tokio::fs::read_to_string(&file_ws2).await?;
    assert_eq!(content_ws1, "pub fn magic() -> u32 { 200 }\n");
    assert_eq!(content_ws2, "pub fn magic() -> u32 { 300 }\n");

    // Parent worktree remains completely unchanged
    let parent_content = tokio::fs::read_to_string(&parent_file).await?;
    assert_eq!(parent_content, "pub fn magic() -> u32 { 100 }\n");

    // Apply child 2's result back to parent
    worktree_mgr.apply_to_base(&ws2).await?;

    // Parent worktree now reflects child 2's changes
    let updated_parent = tokio::fs::read_to_string(&parent_file).await?;
    assert_eq!(updated_parent, "pub fn magic() -> u32 { 300 }\n");

    // Child 1 remains isolated with its own bytes
    assert_eq!(
        tokio::fs::read_to_string(&file_ws1).await?,
        "pub fn magic() -> u32 { 200 }\n"
    );

    // Clean up
    worktree_mgr.cleanup_workspace("child_agent_1").await?;
    worktree_mgr.cleanup_workspace("child_agent_2").await?;

    assert!(!ws1.path.exists());
    assert!(!ws2.path.exists());

    let _ = tokio::fs::remove_dir_all(&test_root).await;
    Ok(())
}
