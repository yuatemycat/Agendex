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

/// Formats a user task prompt into a structured Conventional Commit message.
pub fn format_commit_message(summary: &str) -> String {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        return "chore: task completed via Agendex".to_string();
    }
    if trimmed.starts_with("feat:")
        || trimmed.starts_with("fix:")
        || trimmed.starts_with("chore:")
        || trimmed.starts_with("refactor:")
        || trimmed.starts_with("docs:")
        || trimmed.starts_with("test:")
    {
        return trimmed.to_string();
    }

    let clean = trimmed
        .trim_start_matches("Please ")
        .trim_start_matches("please ")
        .trim_start_matches("Can you ")
        .trim_start_matches("can you ")
        .trim_start_matches("Could you ")
        .trim_start_matches("could you ")
        .trim_start_matches("I need you to ")
        .trim_start_matches("Help me ")
        .trim();

    let clean_lower = clean.to_lowercase();
    let prefix = if clean_lower.starts_with("fix")
        || clean_lower.starts_with("bug")
        || clean_lower.starts_with("patch")
        || clean_lower.starts_with("resolve")
    {
        "fix"
    } else if clean_lower.starts_with("add")
        || clean_lower.starts_with("create")
        || clean_lower.starts_with("implement")
        || clean_lower.starts_with("build")
        || clean_lower.starts_with("new")
    {
        "feat"
    } else if clean_lower.starts_with("refactor")
        || clean_lower.starts_with("rename")
        || clean_lower.starts_with("clean")
    {
        "refactor"
    } else if clean_lower.starts_with("test") {
        "test"
    } else if clean_lower.starts_with("doc") {
        "docs"
    } else {
        "chore"
    };

    let first_line = clean.lines().next().unwrap_or(clean).trim();
    let first_line = first_line.trim_end_matches('.');
    let subject = if first_line.len() > 65 {
        format!("{}: {}", prefix, first_line[..65].trim())
    } else {
        format!("{}: {}", prefix, first_line)
    };

    if trimmed.lines().count() > 1 || trimmed.len() > 72 {
        format!("{}\n\nTask prompt:\n{}", subject, trimmed)
    } else {
        subject
    }
}

/// Resolves Git author identity overrides (user.name and user.email).
/// If not already configured in repository or globally, returns Agendex defaults
/// to be passed as runtime flags/environment variables without mutating the repo's `.git/config`.
pub async fn resolve_git_author_env(workspace_dir: &Path) -> (Option<String>, Option<String>) {
    let name_out = Command::new("git")
        .args(["config", "user.name"])
        .current_dir(workspace_dir)
        .output()
        .await;
    let name_set = name_out
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);

    let email_out = Command::new("git")
        .args(["config", "user.email"])
        .current_dir(workspace_dir)
        .output()
        .await;
    let email_set = email_out
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);

    let author_name = if !name_set {
        Some("Agendex Agent".to_string())
    } else {
        None
    };
    let author_email = if !email_set {
        Some("agent@agendex.local".to_string())
    } else {
        None
    };

    (author_name, author_email)
}

/// Helper to ensure git identity is available without modifying repository configuration.
pub async fn ensure_git_identity(_workspace_dir: &Path) -> Result<()> {
    // Identity is dynamically provided to git commit via runtime args/env,
    // leaving repo config completely untouched.
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
/// If `target_files` is specified, only stages and commits those exact files (never running `git add -A`).
/// If `target_files` is None, stages modified tracked files via `git add -u` to prevent sweeping in unrelated untracked files.
pub async fn commit_task_changes(
    workspace_dir: &Path,
    summary: &str,
    target_files: Option<&[String]>,
) -> Result<Option<GitCommitResult>> {
    let uncommitted = get_uncommitted_files(workspace_dir).await?;
    if uncommitted.is_empty() {
        return Ok(None);
    }

    // 1. Stage only the targeted files, or tracked modifications if none specified
    let mut add_cmd = Command::new("git");
    add_cmd.current_dir(workspace_dir);
    if let Some(files) = target_files {
        let is_build_artifact = |f: &str| {
            let l = f.to_lowercase();
            l.ends_with(".pyc")
                || l.ends_with(".pyo")
                || l.contains("__pycache__")
                || l.contains(".pytest_cache")
                || l.starts_with("target/")
                || l.contains("/target/")
                || l.starts_with(".mypy_cache")
        };
        let valid_files: Vec<&String> = files
            .iter()
            .filter(|f| !f.trim().is_empty() && !is_build_artifact(f))
            .collect();
        if !valid_files.is_empty() {
            add_cmd.arg("add").arg("--");
            for f in valid_files {
                add_cmd.arg(f);
            }
        } else {
            add_cmd.args(["add", "-u"]);
        }
    } else {
        add_cmd.args(["add", "-u"]);
    }

    let add_out = add_cmd
        .output()
        .await
        .map_err(|e| anyhow!("Failed to run git add: {}", e))?;

    if !add_out.status.success() {
        return Err(anyhow!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_out.stderr)
        ));
    }

    // Check what is actually staged in index
    let diff_cached = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(workspace_dir)
        .output()
        .await?;
    let staged_files: Vec<String> = String::from_utf8_lossy(&diff_cached.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if staged_files.is_empty() {
        return Ok(None);
    }

    // 2. Format commit message
    let commit_msg = format_commit_message(summary);

    // 3. Commit changes with author identity without mutating repo config
    let (extra_name, extra_email) = resolve_git_author_env(workspace_dir).await;
    let mut commit_cmd = Command::new("git");
    commit_cmd.current_dir(workspace_dir);
    if let Some(ref name) = extra_name {
        commit_cmd.args(["-c", &format!("user.name={}", name)]);
        commit_cmd.env("GIT_AUTHOR_NAME", name);
        commit_cmd.env("GIT_COMMITTER_NAME", name);
    }
    if let Some(ref email) = extra_email {
        commit_cmd.args(["-c", &format!("user.email={}", email)]);
        commit_cmd.env("GIT_AUTHOR_EMAIL", email);
        commit_cmd.env("GIT_COMMITTER_EMAIL", email);
    }
    commit_cmd.args(["commit", "-m", &commit_msg]);

    let commit_out = commit_cmd
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
        files_changed: staged_files,
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
        let res1 = commit_task_changes(&tmp, "initial commit", Some(&["README.md".to_string()]))
            .await
            .unwrap();
        assert!(res1.is_some());
        let c1 = res1.unwrap();
        assert!(!c1.commit_sha.is_empty());
        assert_eq!(c1.files_changed, vec!["README.md".to_string()]);

        // 3. Clean status returns None
        let res_clean = commit_task_changes(&tmp, "no changes", Some(&["README.md".to_string()]))
            .await
            .unwrap();
        assert!(res_clean.is_none());

        // 4. Create branch
        let br_msg = create_or_checkout_branch(&tmp, "feature/ast-navigation")
            .await
            .unwrap();
        assert!(br_msg.contains("feature/ast-navigation"));

        // 5. Commit changes on new branch (only targets specified files, preserving unrelated untracked files)
        let unrelated_untracked = tmp.join("user_unrelated.txt");
        std::fs::write(&unrelated_untracked, "do not commit me").unwrap();

        std::fs::write(tmp.join("README.md"), "# Updated on feature branch").unwrap();
        std::fs::write(tmp.join("new_file.txt"), "hello").unwrap();
        let res2 = commit_task_changes(
            &tmp,
            "add new feature file",
            Some(&["README.md".to_string(), "new_file.txt".to_string()]),
        )
        .await
        .unwrap();
        assert!(res2.is_some());
        let c2 = res2.unwrap();
        assert_eq!(c2.files_changed.len(), 2);
        assert!(!c2.files_changed.contains(&"user_unrelated.txt".to_string()));
        assert!(c2.message.contains("add new feature file"));

        // 6. When target_files is None, stages tracked updates only without committing untracked files
        std::fs::write(tmp.join("README.md"), "# Updated again").unwrap();
        let res3 = commit_task_changes(&tmp, "tracked update only", None)
            .await
            .unwrap();
        assert!(res3.is_some());
        let c3 = res3.unwrap();
        assert_eq!(c3.files_changed, vec!["README.md".to_string()]);
        assert!(!c3.files_changed.contains(&"user_unrelated.txt".to_string()));

        // Verify user_unrelated.txt is still untracked
        let uncommitted = get_uncommitted_files(&tmp).await.unwrap();
        assert!(
            uncommitted.iter().any(|f| f.contains("user_unrelated.txt")),
            "Untracked file must remain untracked"
        );

        // Verify git config user.name was NOT written into the local repo
        let local_config = std::fs::read_to_string(tmp.join(".git").join("config")).unwrap();
        assert!(
            !local_config.contains("Agendex Agent"),
            "Local git config must NOT be mutated"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
