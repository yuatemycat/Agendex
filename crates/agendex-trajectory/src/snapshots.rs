use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::process::Command;

/// Record representing a workspace tree snapshot at a discrete point in time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SnapshotRecord {
    pub id: String,
    pub label: String,
    pub tree_hash: String,
    pub created_at: chrono::DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Git workspace time-travel engine using lightweight Git tree hashing (`git write-tree`).
pub struct SnapshotEngine {
    workspace_dir: PathBuf,
    snapshots: Vec<SnapshotRecord>,
    git_initialized: bool,
}

impl SnapshotEngine {
    /// Initialize a new snapshot engine for the given workspace directory.
    pub fn new(workspace_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let workspace_dir = workspace_dir.as_ref().to_path_buf();
        let git_dir = workspace_dir.join(".git");
        let git_initialized = git_dir.exists();

        let mut engine = Self {
            workspace_dir,
            snapshots: Vec::new(),
            git_initialized,
        };

        // If .git does not exist, initialize a clean git repository automatically
        if !engine.git_initialized {
            engine.init_git_sync()?;
        }

        Ok(engine)
    }

    /// Synchronously initialize a local git repository in workspace if needed.
    fn init_git_sync(&mut self) -> anyhow::Result<()> {
        let status = std::process::Command::new("git")
            .arg("init")
            .current_dir(&self.workspace_dir)
            .status();

        match status {
            Ok(s) if s.success() => {
                self.git_initialized = true;
                // Configure local user email/name if not present to avoid git warnings
                let _ = std::process::Command::new("git")
                    .args(["config", "user.name", "Agenda Agent"])
                    .current_dir(&self.workspace_dir)
                    .status();
                let _ = std::process::Command::new("git")
                    .args(["config", "user.email", "agent@agenda.local"])
                    .current_dir(&self.workspace_dir)
                    .status();
                let _ = std::process::Command::new("git")
                    .args(["config", "core.autocrlf", "false"])
                    .current_dir(&self.workspace_dir)
                    .status();
                Ok(())
            }
            Ok(s) => anyhow::bail!("git init exited with status: {}", s),
            Err(e) => anyhow::bail!("Failed to execute git init: {}", e),
        }
    }

    /// Asynchronously capture a pre-mutation workspace snapshot using `git write-tree`.
    pub async fn capture_snapshot(&mut self, label: &str) -> anyhow::Result<SnapshotRecord> {
        self.capture_snapshot_with_metadata(label, None).await
    }

    /// Capture snapshot with custom structured metadata attached.
    pub async fn capture_snapshot_with_metadata(
        &mut self,
        label: &str,
        metadata: Option<serde_json::Value>,
    ) -> anyhow::Result<SnapshotRecord> {
        self.ensure_git_ready().await?;

        // 1. Stage all working tree files (git add -A)
        let add_out = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.workspace_dir)
            .output()
            .await?;
        Self::check_output(&add_out, "git add -A")?;

        // 2. Compute the lightweight tree hash (git write-tree)
        let write_tree_out = Command::new("git")
            .arg("write-tree")
            .current_dir(&self.workspace_dir)
            .output()
            .await?;
        Self::check_output(&write_tree_out, "git write-tree")?;

        let tree_hash = String::from_utf8_lossy(&write_tree_out.stdout)
            .trim()
            .to_string();
        if tree_hash.len() != 40 {
            anyhow::bail!(
                "Invalid git tree hash returned by git write-tree: '{}'",
                tree_hash
            );
        }

        let record = SnapshotRecord {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.to_string(),
            tree_hash,
            created_at: Utc::now(),
            metadata,
        };

        self.snapshots.push(record.clone());
        tracing::debug!(
            "Captured workspace snapshot [{}] hash={}",
            record.label,
            record.tree_hash
        );
        Ok(record)
    }

    /// Roll back workspace files to the exact state captured at `tree_hash_or_label`.
    pub async fn rollback(&mut self, tree_hash_or_label: &str) -> anyhow::Result<SnapshotRecord> {
        self.ensure_git_ready().await?;

        // Find matching snapshot: exact tree_hash, id, or latest matching label
        let record = self
            .snapshots
            .iter()
            .rev()
            .find(|s| {
                s.tree_hash == tree_hash_or_label
                    || s.id == tree_hash_or_label
                    || s.label == tree_hash_or_label
            })
            .cloned()
            .or_else(|| {
                // If it is a 40-character hex hash not recorded in memory, allow direct rollback
                if tree_hash_or_label.len() == 40
                    && tree_hash_or_label.chars().all(|c| c.is_ascii_hexdigit())
                {
                    Some(SnapshotRecord {
                        id: uuid::Uuid::new_v4().to_string(),
                        label: format!("manual:{}", tree_hash_or_label),
                        tree_hash: tree_hash_or_label.to_string(),
                        created_at: Utc::now(),
                        metadata: None,
                    })
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Snapshot not found matching tree hash, ID, or label: '{}'",
                    tree_hash_or_label
                )
            })?;

        // 1. Reset index and working directory to the tree snapshot (git read-tree -u --reset <tree_hash>)
        let read_out = Command::new("git")
            .args(["read-tree", "-u", "--reset", &record.tree_hash])
            .current_dir(&self.workspace_dir)
            .output()
            .await?;
        Self::check_output(&read_out, "git read-tree -u --reset")?;

        // 2. Remove untracked files that were introduced after the snapshot was taken (git clean -fd)
        let clean_out = Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(&self.workspace_dir)
            .output()
            .await?;
        Self::check_output(&clean_out, "git clean -fd")?;

        tracing::info!(
            "Workspace successfully rolled back to snapshot [{}] tree_hash={}",
            record.label,
            record.tree_hash
        );
        Ok(record)
    }

    /// Roll back workspace to the most recent snapshot.
    pub async fn rollback_latest(&mut self) -> anyhow::Result<SnapshotRecord> {
        let latest_hash = self
            .snapshots
            .last()
            .map(|s| s.tree_hash.clone())
            .ok_or_else(|| anyhow::anyhow!("No snapshots available to roll back"))?;

        self.rollback(&latest_hash).await
    }

    /// Generate unified diff comparing working tree to a recorded snapshot.
    pub async fn diff_snapshot(&self, tree_hash_or_label: &str) -> anyhow::Result<String> {
        let record = self
            .snapshots
            .iter()
            .rev()
            .find(|s| {
                s.tree_hash == tree_hash_or_label
                    || s.id == tree_hash_or_label
                    || s.label == tree_hash_or_label
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Snapshot not found for diff comparison: '{}'",
                    tree_hash_or_label
                )
            })?;

        let diff_out = Command::new("git")
            .args(["diff", &record.tree_hash])
            .current_dir(&self.workspace_dir)
            .output()
            .await?;

        Self::check_output(&diff_out, "git diff")?;
        Ok(String::from_utf8_lossy(&diff_out.stdout).to_string())
    }

    /// Ensure git repository is initialized.
    async fn ensure_git_ready(&mut self) -> anyhow::Result<()> {
        if !self.git_initialized {
            let status = Command::new("git")
                .arg("init")
                .current_dir(&self.workspace_dir)
                .status()
                .await?;
            if status.success() {
                self.git_initialized = true;
            } else {
                anyhow::bail!(
                    "git init failed in workspace: {}",
                    self.workspace_dir.display()
                );
            }
        }
        Ok(())
    }

    /// Validate command output and return informative error if execution failed.
    fn check_output(output: &Output, cmd_desc: &str) -> anyhow::Result<()> {
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "Command '{}' failed with code {:?}: stderr: {}, stdout: {}",
                cmd_desc,
                output.status.code(),
                stderr.trim(),
                stdout.trim()
            );
        }
        Ok(())
    }

    /// List all captured snapshot records in chronological order.
    pub fn list_snapshots(&self) -> &[SnapshotRecord] {
        &self.snapshots
    }

    /// Get the most recent snapshot record.
    pub fn latest_snapshot(&self) -> Option<&SnapshotRecord> {
        self.snapshots.last()
    }

    /// Get a snapshot by tree_hash, id, or label.
    pub fn get_snapshot(&self, tree_hash_or_label: &str) -> Option<&SnapshotRecord> {
        self.snapshots.iter().rev().find(|s| {
            s.tree_hash == tree_hash_or_label
                || s.id == tree_hash_or_label
                || s.label == tree_hash_or_label
        })
    }

    /// Total count of captured snapshots.
    pub fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Clear in-memory snapshot history.
    pub fn clear_history(&mut self) {
        self.snapshots.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_snapshot_capture_and_rollback() -> anyhow::Result<()> {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_test_snap_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await?;

        // Create an initial file
        let file_a = temp_dir.join("file_a.txt");
        tokio::fs::write(&file_a, "Version 1 content\n").await?;

        let mut engine = SnapshotEngine::new(&temp_dir)?;

        // Capture initial snapshot
        let snap1 = engine.capture_snapshot("initial_state").await?;
        assert_eq!(snap1.label, "initial_state");
        assert_eq!(snap1.tree_hash.len(), 40);

        // Mutate existing file and add new file
        tokio::fs::write(&file_a, "Version 2 mutated\n").await?;
        let file_b = temp_dir.join("file_b.txt");
        tokio::fs::write(&file_b, "Ephemeral file to be cleaned\n").await?;

        // Capture second snapshot
        let snap2 = engine.capture_snapshot("mutated_state").await?;
        assert_eq!(snap2.label, "mutated_state");
        assert_ne!(snap1.tree_hash, snap2.tree_hash);

        // Verify diff between working tree and initial_state
        let diff = engine.diff_snapshot("initial_state").await?;
        assert!(diff.contains("Version 2 mutated"));

        // Roll back to initial state
        let restored = engine.rollback("initial_state").await?;
        assert_eq!(restored.tree_hash, snap1.tree_hash);

        // Verify file_a is restored to Version 1
        let content_a = tokio::fs::read_to_string(&file_a).await?;
        assert_eq!(content_a.replace("\r\n", "\n"), "Version 1 content\n");

        // Verify file_b was deleted (via git clean -fd)
        assert!(!file_b.exists());

        // Test rollback to snap2
        let restored2 = engine.rollback(&snap2.tree_hash).await?;
        assert_eq!(restored2.tree_hash, snap2.tree_hash);
        let content_a_v2 = tokio::fs::read_to_string(&file_a).await?;
        assert_eq!(content_a_v2.replace("\r\n", "\n"), "Version 2 mutated\n");
        assert!(file_b.exists());

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_snapshot_by_label_and_rollback_latest() -> anyhow::Result<()> {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_test_snap_label_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await?;

        let doc = temp_dir.join("doc.md");
        tokio::fs::write(&doc, "# Baseline\n").await?;

        let mut engine = SnapshotEngine::new(&temp_dir)?;
        let s1 = engine.capture_snapshot("checkpoint-1").await?;
        assert_eq!(engine.snapshot_count(), 1);
        assert_eq!(engine.latest_snapshot().unwrap().label, "checkpoint-1");

        tokio::fs::write(&doc, "# Step 2 edit\n").await?;
        let s2 = engine.capture_snapshot("checkpoint-2").await?;
        assert_eq!(engine.snapshot_count(), 2);

        tokio::fs::write(&doc, "# Step 3 edit\n").await?;

        // Roll back to checkpoint-1 by label
        let restored_1 = engine.rollback("checkpoint-1").await?;
        assert_eq!(restored_1.id, s1.id);
        let content = tokio::fs::read_to_string(&doc).await?;
        assert_eq!(content.replace("\r\n", "\n"), "# Baseline\n");

        // Roll back latest (which was s2)
        let restored_latest = engine.rollback_latest().await?;
        assert_eq!(restored_latest.id, s2.id);
        let content_latest = tokio::fs::read_to_string(&doc).await?;
        assert_eq!(content_latest.replace("\r\n", "\n"), "# Step 2 edit\n");

        // Rollback non-existent label fails cleanly
        let err = engine.rollback("non_existent_label").await;
        assert!(err.is_err());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }
}
