use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;

/// Details of a committed workspace state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitCommitResult {
    pub commit_sha: String,
    pub message: String,
    pub files_changed: Vec<String>,
}

/// Ensure Git author identity (user.name and user.email) is configured in repository.
pub async fn ensure_git_identity(workspace_dir: &Path) -> Result<()> {
    // Check if user.name is set
    let name_out = Command::new("git")
        .args(["config", "user.name"])
        .current_dir(workspace_dir)
        .output()
        .await?;

    if name_out.stdout.is_empty() || !name_out.status.success() {
        let _ = Command::new("git")
            .args(["config", "user.name", "Agendex Agent"])
            .current_dir(workspace_dir)
            .status()
            .await;
    }

    let email_out = Command::new("git")
        .args(["config", "user.email"])
        .current_dir(workspace_dir)
        .output()
        .await?;

    if email_out.stdout.is_empty() || !email_out.status.success() {
        let _ = Command::new("git")
            .args(["config", "user.email", "agent@agendex.local"])
            .current_dir(workspace_dir)
            .status()
            .await;
    }

    Ok(())
}

/// Returns a list of modified, added, or untracked files in the workspace.
pub async fn get_uncommitted_files(workspace_dir: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace_dir)
        .output()
        .await
        .map_err(|e| anyhow!("Failed to execute 'git status --porcelain': {}", e))?;

    if !out.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut files = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.len() > 3 {
            let file_part = line[3..].trim();
            if let Some((_, new_file)) = file_part.split_once("->") {
                files.push(new_file.trim().to_string());
            } else {
                files.push(file_part.to_string());
            }
        }
    }
    Ok(files)
}

/// Helper to create and/or checkout a Git branch for the task.
pub async fn create_or_checkout_branch(workspace_dir: &Path, branch_name: &str) -> Result<String> {
    ensure_git_identity(workspace_dir).await?;

    // Try checking out existing branch first
    let checkout_out = Command::new("git")
        .args(["checkout", branch_name])
        .current_dir(workspace_dir)
        .output()
        .await?;

    if checkout_out.status.success() {
        return Ok(format!("Switched to existing branch '{}'", branch_name));
    }

    // Branch doesn't exist yet, create and switch to it
    let create_out = Command::new("git")
        .args(["checkout", "-b", branch_name])
        .current_dir(workspace_dir)
        .output()
        .await
        .map_err(|e| anyhow!("Failed to run 'git checkout -b': {}", e))?;

    if !create_out.status.success() {
        return Err(anyhow!(
            "Failed to create branch '{}': {}",
            branch_name,
            String::from_utf8_lossy(&create_out.stderr)
        ));
    }

    Ok(format!("Created and switched to branch '{}'", branch_name))
}

/// Automatically stages and commits workspace changes with a descriptive summary.
/// Returns None if there are no changes to commit.
pub async fn commit_task_changes(
    workspace_dir: &Path,
    summary: &str,
) -> Result<Option<GitCommitResult>> {
    let uncommitted = get_uncommitted_files(workspace_dir).await?;
    if uncommitted.is_empty() {
        return Ok(None);
    }

    ensure_git_identity(workspace_dir).await?;

    // 1. Stage all changes
    let add_out = Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace_dir)
        .output()
        .await
        .map_err(|e| anyhow!("Failed to run 'git add -A': {}", e))?;

    if !add_out.status.success() {
        return Err(anyhow!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add_out.stderr)
        ));
    }

    // 2. Format commit message
    let trimmed = summary.trim();
    let commit_msg = if trimmed.is_empty() {
        "chore: task completed via Agendex".to_string()
    } else if trimmed.starts_with("feat:")
        || trimmed.starts_with("fix:")
        || trimmed.starts_with("chore:")
        || trimmed.starts_with("agendex:")
    {
        trimmed.to_string()
    } else {
        let first_line = trimmed.lines().next().unwrap_or("task completed").trim();
        if first_line.len() > 72 {
            format!("agendex: {}\n\n{}", &first_line[..72], trimmed)
        } else {
            format!("agendex: {}", trimmed)
        }
    };

    // 3. Commit changes
    let commit_out = Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .current_dir(workspace_dir)
        .output()
        .await
        .map_err(|e| anyhow!("Failed to run 'git commit': {}", e))?;

    if !commit_out.status.success() {
        return Err(anyhow!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit_out.stderr)
        ));
    }

    // 4. Retrieve commit SHA
    let rev_out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_dir)
        .output()
        .await
        .map_err(|e| anyhow!("Failed to run 'git rev-parse HEAD': {}", e))?;

    let commit_sha = String::from_utf8_lossy(&rev_out.stdout).trim().to_string();

    Ok(Some(GitCommitResult {
        commit_sha,
        message: commit_msg,
        files_changed: uncommitted,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_git_helpers_flow() {
        let tmp = std::env::temp_dir().join(format!(
            "agendex_git_test_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        // 1. Initialize git repo
        let status = Command::new("git")
            .arg("init")
            .current_dir(&tmp)
            .status()
            .await
            .unwrap();
        assert!(status.success());

        // 2. Initial commit
        ensure_git_identity(&tmp).await.unwrap();
        std::fs::write(tmp.join("README.md"), "# Initial").unwrap();
        let res1 = commit_task_changes(&tmp, "initial commit").await.unwrap();
        assert!(res1.is_some());
        let c1 = res1.unwrap();
        assert!(!c1.commit_sha.is_empty());
        assert_eq!(c1.files_changed, vec!["README.md".to_string()]);

        // 3. Clean status returns None
        let res_clean = commit_task_changes(&tmp, "no changes").await.unwrap();
        assert!(res_clean.is_none());

        // 4. Create branch
        let br_msg = create_or_checkout_branch(&tmp, "feature/ast-navigation")
            .await
            .unwrap();
        assert!(br_msg.contains("feature/ast-navigation"));

        // 5. Commit changes on new branch
        std::fs::write(tmp.join("README.md"), "# Updated on feature branch").unwrap();
        std::fs::write(tmp.join("new_file.txt"), "hello").unwrap();
        let res2 = commit_task_changes(&tmp, "add new feature file")
            .await
            .unwrap();
        assert!(res2.is_some());
        let c2 = res2.unwrap();
        assert_eq!(c2.files_changed.len(), 2);
        assert!(c2.message.contains("add new feature file"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
