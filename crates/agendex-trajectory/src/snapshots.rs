use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::process::Command;

/// Resolves the dedicated Agendex state directory for a workspace.
///
/// Priority:
/// 1. `AGENDEX_STATE_DIR` environment variable if set.
/// 2. If `.git` exists in `workspace_dir` (as a directory or gitdir pointer), stores under `.git/agendex/`.
/// 3. Otherwise, stores under the user's home folder: `~/.agendex/workspaces/<sanitized-name>-<hash>/`.
pub fn state_dir_for_workspace(workspace_dir: &Path) -> PathBuf {
    if let Ok(custom) = std::env::var("AGENDEX_STATE_DIR") {
        if !custom.trim().is_empty() {
            return PathBuf::from(custom.trim());
        }
    }

    let git_path = workspace_dir.join(".git");
    if git_path.is_dir() {
        return git_path.join("agendex");
    }

    if git_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&git_path) {
            if let Some(rest) = content.trim().strip_prefix("gitdir:") {
                let p = rest.trim();
                let real_git_dir = if Path::new(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    workspace_dir.join(p)
                };
                return real_git_dir.join("agendex");
            }
        }
        return git_path.join("agendex");
    }

    // Fall back to home folder
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);

    use std::hash::{Hash, Hasher};
    let canonical = workspace_dir
        .canonicalize()
        .unwrap_or_else(|_| workspace_dir.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    canonical.to_string_lossy().hash(&mut hasher);
    let hash_val = format!("{:012x}", hasher.finish());

    let folder_name = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");

    home.join(".agendex")
        .join("workspaces")
        .join(format!("{folder_name}-{hash_val}"))
}

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
    state_dir: PathBuf,
    index_file: PathBuf,
    snapshots_file: PathBuf,
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
            state_dir: PathBuf::new(),
            index_file: PathBuf::new(),
            snapshots_file: PathBuf::new(),
            snapshots: Vec::new(),
            git_initialized,
        };

        // If .git does not exist, initialize a clean git repository automatically
        if !engine.git_initialized {
            engine.init_git_sync()?;
        }

        let state_dir = state_dir_for_workspace(&engine.workspace_dir);
        engine.init_state_dir(state_dir)?;

        Ok(engine)
    }

    /// Initialize a new snapshot engine with a specific state directory.
    pub fn new_with_state_dir(
        workspace_dir: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let workspace_dir = workspace_dir.as_ref().to_path_buf();
        let git_dir = workspace_dir.join(".git");
        let git_initialized = git_dir.exists();

        let mut engine = Self {
            workspace_dir,
            state_dir: PathBuf::new(),
            index_file: PathBuf::new(),
            snapshots_file: PathBuf::new(),
            snapshots: Vec::new(),
            git_initialized,
        };

        if !engine.git_initialized {
            engine.init_git_sync()?;
        }

        engine.init_state_dir(state_dir.as_ref().to_path_buf())?;

        Ok(engine)
    }

    /// Configure state directory, isolated index path, and load persistent snapshots from disk.
    fn init_state_dir(&mut self, state_dir: PathBuf) -> anyhow::Result<()> {
        std::fs::create_dir_all(&state_dir)?;
        self.index_file = state_dir.join("index");
        self.snapshots_file = state_dir.join("snapshots.json");
        self.state_dir = state_dir;

        if self.snapshots_file.is_file() {
            if let Ok(data) = std::fs::read_to_string(&self.snapshots_file) {
                if let Ok(records) = serde_json::from_str::<Vec<SnapshotRecord>>(&data) {
                    self.snapshots = records;
                }
            }
        }
        Ok(())
    }

    /// Persist in-memory snapshot records to disk so `/revert` works across CLI invocations.
    pub fn save_snapshots_to_disk(&self) -> anyhow::Result<()> {
        if !self.snapshots_file.as_os_str().is_empty() {
            if let Some(parent) = self.snapshots_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let data = serde_json::to_string_pretty(&self.snapshots)?;
            std::fs::write(&self.snapshots_file, data)?;
        }
        Ok(())
    }

    /// Path to the dedicated state directory.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Path to the isolated index file used to prevent modifying user's real `.git/index`.
    pub fn index_file(&self) -> &Path {
        &self.index_file
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

        // 1. Stage working tree files using isolated index (NEVER modifying user's real .git/index)
        let mut cmd = Command::new("git");
        cmd.args(["add", "-A"]).current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let add_out = cmd.output().await?;
        Self::check_output(&add_out, "git add -A (isolated index)")?;

        // 2. Compute lightweight tree hash from isolated index (git write-tree)
        let mut wt_cmd = Command::new("git");
        wt_cmd.arg("write-tree").current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            wt_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let write_tree_out = wt_cmd.output().await?;
        Self::check_output(&write_tree_out, "git write-tree (isolated index)")?;

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
        let _ = self.save_snapshots_to_disk();

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

        // 1. Reset isolated index and working directory to the tree snapshot (git read-tree -u --reset <tree_hash>)
        // Note: We deliberately do NOT run `git clean -fd`. Untracked files (such as user-created
        // test files) must never be destroyed by rollback. Files that were tracked in later
        // snapshots are cleanly removed by `read-tree -u --reset` because they exist in the index.
        let mut read_cmd = Command::new("git");
        read_cmd
            .args(["read-tree", "-u", "--reset", &record.tree_hash])
            .current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            read_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let read_out = read_cmd.output().await?;
        Self::check_output(&read_out, "git read-tree -u --reset")?;

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

        let mut diff_cmd = Command::new("git");
        diff_cmd
            .args(["diff", &record.tree_hash])
            .current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            diff_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let diff_out = diff_cmd.output().await?;

        Self::check_output(&diff_out, "git diff (isolated index)")?;
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
                if self.state_dir.as_os_str().is_empty() {
                    let state_dir = state_dir_for_workspace(&self.workspace_dir);
                    self.init_state_dir(state_dir)?;
                }
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

    /// Clear in-memory snapshot history and update disk storage.
    pub fn clear_history(&mut self) {
        self.snapshots.clear();
        let _ = self.save_snapshots_to_disk();
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

    #[tokio::test]
    async fn test_snapshot_disk_persistence_and_isolated_index() -> anyhow::Result<()> {
        let temp_dir = std::env::temp_dir().join(format!(
            "agendex_test_snap_persist_{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&temp_dir).await?;

        // 1. Initialize repo with a committed file
        let mut engine1 = SnapshotEngine::new(&temp_dir)?;
        let file1 = temp_dir.join("tracked.txt");
        tokio::fs::write(&file1, "Initial tracked content\n").await?;

        // 2. Stage a user file in real git index
        let user_staged = temp_dir.join("user_staged.txt");
        tokio::fs::write(&user_staged, "User explicitly staged this\n").await?;
        let stage_out = Command::new("git")
            .args(["add", "user_staged.txt"])
            .current_dir(&temp_dir)
            .output()
            .await?;
        assert!(stage_out.status.success());

        // Check real git status before snapshot
        let status_pre = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&temp_dir)
            .output()
            .await?;
        let status_pre_str = String::from_utf8_lossy(&status_pre.stdout).to_string();
        assert!(status_pre_str.contains("A  user_staged.txt"));
        assert!(status_pre_str.contains("?? tracked.txt"));

        // 3. Engine captures snapshot (MUST NOT stage tracked.txt into real git index)
        let snap1 = engine1.capture_snapshot("turn-1").await?;
        assert_eq!(snap1.label, "turn-1");

        // Real git status MUST still have tracked.txt as untracked in real index
        let status_post = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&temp_dir)
            .output()
            .await?;
        let status_post_str = String::from_utf8_lossy(&status_post.stdout).to_string();
        assert_eq!(
            status_pre_str, status_post_str,
            "Real git status must not be modified by snapshot capture"
        );

        // 4. Verify snapshots.json was written to disk
        let snapshots_json = engine1.state_dir().join("snapshots.json");
        assert!(snapshots_json.exists(), "snapshots.json must exist on disk");

        // 5. Drop engine1, simulate a subsequent -p or resumed session with engine2
        drop(engine1);
        let mut engine2 = SnapshotEngine::new(&temp_dir)?;
        assert_eq!(
            engine2.snapshot_count(),
            1,
            "engine2 must reload snapshots from disk"
        );
        assert_eq!(engine2.latest_snapshot().unwrap().id, snap1.id);

        // 6. Mutate file and rollback via engine2
        tokio::fs::write(&file1, "Corrupted by agent\n").await?;
        let user_handmade_test = temp_dir.join("user_test.txt");
        tokio::fs::write(&user_handmade_test, "Hand-made user test file\n").await?;

        let rolled = engine2.rollback_latest().await?;
        assert_eq!(rolled.id, snap1.id);

        assert_eq!(
            tokio::fs::read_to_string(&file1)
                .await?
                .replace("\r\n", "\n"),
            "Initial tracked content\n"
        );
        assert!(
            user_handmade_test.exists(),
            "User hand-made test file must NEVER be deleted by rollback"
        );

        // Real user staged file is still preserved
        let status_after_revert = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&temp_dir)
            .output()
            .await?;
        let status_after_revert_str =
            String::from_utf8_lossy(&status_after_revert.stdout).to_string();
        assert!(status_after_revert_str.contains("A  user_staged.txt"));

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }
}
