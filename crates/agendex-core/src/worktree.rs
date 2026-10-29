use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Workspace isolation mode for subagent execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorkspaceMode {
    /// Inherits the parent workspace directly (shared filesystem, single writer).
    #[default]
    Inherit,
    /// Creates an isolated filesystem copy/sandbox in a dedicated directory.
    Branch,
    /// Uses native Git worktree isolation (`git worktree add`) sharing repository history.
    Share,
}

/// Representation of an active isolated workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IsolatedWorkspace {
    pub id: String,
    pub mode: WorkspaceMode,
    pub path: PathBuf,
    pub base_repo_path: PathBuf,
    pub branch_name: Option<String>,
    pub is_cleaned_up: bool,
}

/// Manages isolated workspaces and Git worktrees for subagents.
pub struct WorktreeManager {
    pub base_repo_path: PathBuf,
    pub worktree_root: PathBuf,
    workspaces: Arc<RwLock<HashMap<String, IsolatedWorkspace>>>,
}

impl WorktreeManager {
    /// Initialize a worktree manager for the given repository or project directory.
    pub fn new(base_repo_path: PathBuf, worktree_root: Option<PathBuf>) -> anyhow::Result<Self> {
        let worktree_root =
            worktree_root.unwrap_or_else(|| base_repo_path.join(".agendex").join("worktrees"));

        // Ensure worktree root directory exists
        if !worktree_root.exists() {
            std::fs::create_dir_all(&worktree_root)?;
        }

        Ok(Self {
            base_repo_path,
            worktree_root,
            workspaces: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Ensure the base directory has a valid Git repository with at least one commit.
    fn ensure_git_initialized(&self) -> anyhow::Result<()> {
        let has_git = self.base_repo_path.join(".git").exists();
        if !has_git {
            let status = Command::new("git")
                .arg("init")
                .current_dir(&self.base_repo_path)
                .status()?;
            if !status.success() {
                anyhow::bail!(
                    "Failed to initialize Git repository in {}",
                    self.base_repo_path.display()
                );
            }

            // Configure dummy author if needed
            let _ = Command::new("git")
                .args(["config", "user.name", "Agenda Agent"])
                .current_dir(&self.base_repo_path)
                .status();
            let _ = Command::new("git")
                .args(["config", "user.email", "agent@agendex.local"])
                .current_dir(&self.base_repo_path)
                .status();
        }

        // Stage and commit any existing files so HEAD contains full workspace files for worktree checkout
        let _ = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.base_repo_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        let _ = Command::new("git")
            .args([
                "commit",
                "-m",
                "chore: sync files for worktree initialization",
                "--allow-empty",
            ])
            .current_dir(&self.base_repo_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        Ok(())
    }

    /// Create an isolated workspace according to the specified mode.
    pub async fn create_workspace(
        &self,
        id: &str,
        mode: WorkspaceMode,
    ) -> anyhow::Result<IsolatedWorkspace> {
        match mode {
            WorkspaceMode::Inherit => {
                let ws = IsolatedWorkspace {
                    id: id.to_string(),
                    mode,
                    path: self.base_repo_path.clone(),
                    base_repo_path: self.base_repo_path.clone(),
                    branch_name: None,
                    is_cleaned_up: false,
                };
                let mut map = self.workspaces.write().await;
                map.insert(id.to_string(), ws.clone());
                Ok(ws)
            }
            WorkspaceMode::Branch => {
                let dest_path = self.worktree_root.join(format!("branch_{}", id));
                if dest_path.exists() {
                    let _ = tokio::fs::remove_dir_all(&dest_path).await;
                }
                tokio::fs::create_dir_all(&dest_path).await?;

                // Recursively copy project files into isolated sandbox, excluding heavy folders
                copy_dir_recursive(
                    &self.base_repo_path,
                    &dest_path,
                    &[".git", "target", ".agendex", ".agenda"],
                )?;

                let ws = IsolatedWorkspace {
                    id: id.to_string(),
                    mode,
                    path: dest_path,
                    base_repo_path: self.base_repo_path.clone(),
                    branch_name: None,
                    is_cleaned_up: false,
                };
                let mut map = self.workspaces.write().await;
                map.insert(id.to_string(), ws.clone());
                Ok(ws)
            }
            WorkspaceMode::Share => {
                self.ensure_git_initialized()?;
                let dest_path = self.worktree_root.join(format!("share_{}", id));
                let branch_name = format!("agendex-wt-{}", id);

                if dest_path.exists() {
                    // Try to remove previous worktree if any
                    let _ = Command::new("git")
                        .args([
                            "worktree",
                            "remove",
                            "--force",
                            dest_path.to_str().unwrap_or(""),
                        ])
                        .current_dir(&self.base_repo_path)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                    let _ = tokio::fs::remove_dir_all(&dest_path).await;
                }

                // Delete branch if already exists
                let _ = Command::new("git")
                    .args(["branch", "-D", &branch_name])
                    .current_dir(&self.base_repo_path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();

                let output = Command::new("git")
                    .args([
                        "worktree",
                        "add",
                        "-b",
                        &branch_name,
                        dest_path.to_str().unwrap_or(""),
                        "HEAD",
                    ])
                    .current_dir(&self.base_repo_path)
                    .output()?;

                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("Failed to create Git worktree: {}", err);
                }

                let ws = IsolatedWorkspace {
                    id: id.to_string(),
                    mode,
                    path: dest_path,
                    base_repo_path: self.base_repo_path.clone(),
                    branch_name: Some(branch_name),
                    is_cleaned_up: false,
                };
                let mut map = self.workspaces.write().await;
                map.insert(id.to_string(), ws.clone());
                Ok(ws)
            }
        }
    }

    /// Retrieve an isolated workspace by ID.
    pub async fn get_workspace(&self, id: &str) -> Option<IsolatedWorkspace> {
        let map = self.workspaces.read().await;
        map.get(id).cloned()
    }

    /// List all currently tracked workspaces.
    pub async fn list_workspaces(&self) -> Vec<IsolatedWorkspace> {
        let map = self.workspaces.read().await;
        map.values().cloned().collect()
    }

    /// Safely clean up and remove an isolated workspace.
    pub async fn cleanup_workspace(&self, id: &str) -> anyhow::Result<()> {
        let ws_opt = {
            let mut map = self.workspaces.write().await;
            map.remove(id)
        };

        if let Some(mut ws) = ws_opt {
            if ws.is_cleaned_up {
                return Ok(());
            }

            match ws.mode {
                WorkspaceMode::Inherit => {
                    // Do nothing for inherited workspace
                }
                WorkspaceMode::Branch => {
                    if ws.path.exists() {
                        let _ = tokio::fs::remove_dir_all(&ws.path).await;
                    }
                }
                WorkspaceMode::Share => {
                    if let Some(branch) = &ws.branch_name {
                        let _ = Command::new("git")
                            .args([
                                "worktree",
                                "remove",
                                "--force",
                                ws.path.to_str().unwrap_or(""),
                            ])
                            .current_dir(&self.base_repo_path)
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status();

                        let _ = Command::new("git")
                            .args(["branch", "-D", branch])
                            .current_dir(&self.base_repo_path)
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status();
                    }

                    if ws.path.exists() {
                        let _ = tokio::fs::remove_dir_all(&ws.path).await;
                    }
                }
            }

            ws.is_cleaned_up = true;
        }

        Ok(())
    }

    /// Clean up all tracked isolated workspaces.
    pub async fn cleanup_all(&self) -> anyhow::Result<()> {
        let ids: Vec<String> = {
            let map = self.workspaces.read().await;
            map.keys().cloned().collect()
        };

        for id in ids {
            let _ = self.cleanup_workspace(&id).await;
        }

        Ok(())
    }

    /// Compute the unified Git diff generated in an isolated workspace against HEAD.
    pub fn get_diff(&self, workspace: &IsolatedWorkspace) -> anyhow::Result<String> {
        if !workspace.path.exists() {
            return Ok(String::new());
        }

        let output = Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&workspace.path)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let diff_str = String::from_utf8_lossy(&out.stdout).to_string();
                if !diff_str.trim().is_empty() {
                    return Ok(diff_str);
                }
            }
            _ => {}
        }

        // If git diff is empty or unavailable, inspect file system changes
        let mut diff_summary = String::new();
        let changed_files = find_modified_files(&workspace.path, &self.base_repo_path)?;
        for file in changed_files {
            diff_summary.push_str(&format!("Modified: {}\n", file.display()));
        }

        Ok(diff_summary)
    }

    /// Apply modifications made in an isolated workspace back onto the base repository.
    pub async fn apply_to_base(&self, workspace: &IsolatedWorkspace) -> anyhow::Result<()> {
        if workspace.mode == WorkspaceMode::Inherit {
            // Already in base repo
            return Ok(());
        }

        if !workspace.path.exists() {
            anyhow::bail!(
                "Workspace path does not exist: {}",
                workspace.path.display()
            );
        }

        // Copy modified and new files from workspace to base repo
        copy_modified_files_to_base(
            &workspace.path,
            &self.base_repo_path,
            &[".git", "target", ".agendex", ".agenda"],
        )?;

        Ok(())
    }
}

/// Recursively copy directory tree, skipping excluded names.
fn copy_dir_recursive(src: &Path, dst: &Path, exclusions: &[&str]) -> anyhow::Result<()> {
    if !src.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if exclusions.iter().any(|ex| *ex == name_str) {
            continue;
        }

        let child_src = entry.path();
        let child_dst = dst.join(&file_name);

        if child_src.is_dir() {
            std::fs::create_dir_all(&child_dst)?;
            copy_dir_recursive(&child_src, &child_dst, exclusions)?;
        } else if child_src.is_file() {
            std::fs::copy(&child_src, &child_dst)?;
        }
    }

    Ok(())
}

/// Compare files in workspace with base repository to detect changes.
fn find_modified_files(ws_dir: &Path, base_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut modified = Vec::new();
    find_modified_internal(ws_dir, ws_dir, base_dir, &mut modified)?;
    Ok(modified)
}

fn find_modified_internal(
    root: &Path,
    current: &Path,
    base_root: &Path,
    results: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    if !current.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if [".git", "target", ".agendex", ".agenda"].contains(&name_str.as_ref()) {
            continue;
        }

        if path.is_dir() {
            find_modified_internal(root, &path, base_root, results)?;
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let base_file = base_root.join(rel);
                if !base_file.exists() {
                    results.push(rel.to_path_buf());
                } else if let (Ok(ws_bytes), Ok(base_bytes)) =
                    (std::fs::read(&path), std::fs::read(&base_file))
                {
                    if ws_bytes != base_bytes {
                        results.push(rel.to_path_buf());
                    }
                }
            }
        }
    }

    Ok(())
}

/// Copy modified or new files from isolated workspace to base repository.
fn copy_modified_files_to_base(
    src_root: &Path,
    dst_root: &Path,
    exclusions: &[&str],
) -> anyhow::Result<()> {
    let modified = find_modified_files(src_root, dst_root)?;
    for rel_path in modified {
        let name = rel_path.to_string_lossy();
        if exclusions.iter().any(|ex| name.contains(ex)) {
            continue;
        }
        let src_file = src_root.join(&rel_path);
        let dst_file = dst_root.join(&rel_path);

        if let Some(parent) = dst_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if src_file.exists() {
            std::fs::copy(&src_file, &dst_file)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_worktree_modes_and_isolation() -> anyhow::Result<()> {
        let test_root =
            std::env::temp_dir().join(format!("agendex_wt_test_{}", uuid::Uuid::new_v4()));
        let base_repo = test_root.join("base_repo");
        let wt_root = test_root.join("worktrees");
        std::fs::create_dir_all(&base_repo)?;

        // Seed initial file
        let initial_file = base_repo.join("hello.txt");
        std::fs::write(&initial_file, "original content")?;

        let manager = WorktreeManager::new(base_repo.clone(), Some(wt_root))?;

        // 1. Inherit mode
        let inherit_ws = manager
            .create_workspace("test_inherit", WorkspaceMode::Inherit)
            .await?;
        assert_eq!(inherit_ws.path, base_repo);

        // 2. Branch mode
        let branch_ws = manager
            .create_workspace("test_branch", WorkspaceMode::Branch)
            .await?;
        assert_ne!(branch_ws.path, base_repo);
        let branch_file = branch_ws.path.join("hello.txt");
        assert!(branch_file.exists());
        assert_eq!(std::fs::read_to_string(&branch_file)?, "original content");

        // Mutate in branch; base should remain untouched
        std::fs::write(&branch_file, "modified in branch")?;
        assert_eq!(std::fs::read_to_string(&initial_file)?, "original content");

        // Apply changes to base
        manager.apply_to_base(&branch_ws).await?;
        assert_eq!(
            std::fs::read_to_string(&initial_file)?,
            "modified in branch"
        );

        // Cleanup
        manager.cleanup_workspace("test_branch").await?;
        assert!(!branch_ws.path.exists());

        // Clean up test root
        let _ = std::fs::remove_dir_all(&test_root);
        Ok(())
    }

    #[tokio::test]
    async fn test_git_share_worktree_isolation() -> anyhow::Result<()> {
        let test_root =
            std::env::temp_dir().join(format!("agendex_wt_share_{}", uuid::Uuid::new_v4()));
        let base_repo = test_root.join("repo");
        let wt_root = test_root.join("worktrees");
        std::fs::create_dir_all(&base_repo)?;

        let initial_file = base_repo.join("code.rs");
        std::fs::write(&initial_file, "pub fn value() -> i32 { 0 }\n")?;

        let manager = WorktreeManager::new(base_repo.clone(), Some(wt_root))?;

        // Create two Share worktrees
        let ws1 = manager
            .create_workspace("child_1", WorkspaceMode::Share)
            .await?;
        let ws2 = manager
            .create_workspace("child_2", WorkspaceMode::Share)
            .await?;

        assert!(ws1.path.exists());
        assert!(ws2.path.exists());
        assert_ne!(ws1.path, ws2.path);

        // Write different bytes in ws1 and ws2
        std::fs::write(ws1.path.join("code.rs"), "pub fn value() -> i32 { 10 }\n")?;
        std::fs::write(ws2.path.join("code.rs"), "pub fn value() -> i32 { 20 }\n")?;

        // Verify isolation
        assert_eq!(
            std::fs::read_to_string(ws1.path.join("code.rs"))?,
            "pub fn value() -> i32 { 10 }\n"
        );
        assert_eq!(
            std::fs::read_to_string(ws2.path.join("code.rs"))?,
            "pub fn value() -> i32 { 20 }\n"
        );
        // Base repo should still have original
        assert_eq!(
            std::fs::read_to_string(&initial_file)?,
            "pub fn value() -> i32 { 0 }\n"
        );

        // Apply child 2 to base
        manager.apply_to_base(&ws2).await?;
        assert_eq!(
            std::fs::read_to_string(&initial_file)?,
            "pub fn value() -> i32 { 20 }\n"
        );

        // Clean up both
        manager.cleanup_workspace("child_1").await?;
        manager.cleanup_workspace("child_2").await?;

        assert!(!ws1.path.exists());
        assert!(!ws2.path.exists());

        let _ = std::fs::remove_dir_all(&test_root);
        Ok(())
    }
}
