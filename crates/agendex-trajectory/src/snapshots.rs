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
    let home = std::env::var("AGENDEX_HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
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

/// Normalizes a path to a workspace-relative path string with forward slashes.
/// Handles absolute paths, verbatim prefixes (\\?\ or //?/), and already-relative paths.
pub fn normalize_rel_path(path: &str, workspace_dir: &Path) -> String {
    let raw = path.trim().replace('\\', "/");
    let unescaped = raw
        .strip_prefix("//?/")
        .or_else(|| raw.strip_prefix("/?//"))
        .unwrap_or(&raw);

    // Normalize workspace directory path
    let ws_str = workspace_dir.to_string_lossy().replace('\\', "/");
    let ws_clean = ws_str
        .strip_prefix("//?/")
        .unwrap_or(&ws_str)
        .trim_end_matches('/');

    // 1. Direct string prefix strip (case-insensitive for Windows drive letters)
    if let Some(suffix) = unescaped.strip_prefix(ws_clean) {
        return suffix.trim_start_matches('/').to_string();
    }
    if unescaped
        .to_lowercase()
        .starts_with(&ws_clean.to_lowercase())
    {
        let suffix = &unescaped[ws_clean.len()..];
        return suffix.trim_start_matches('/').to_string();
    }

    // 2. Canonicalized prefix strip
    if let Ok(canon_ws) = workspace_dir.canonicalize() {
        let canon_ws_str = canon_ws.to_string_lossy().replace('\\', "/");
        let canon_ws_clean = canon_ws_str
            .strip_prefix("//?/")
            .unwrap_or(&canon_ws_str)
            .trim_end_matches('/');
        if let Some(suffix) = unescaped.strip_prefix(canon_ws_clean) {
            return suffix.trim_start_matches('/').to_string();
        }
        if unescaped
            .to_lowercase()
            .starts_with(&canon_ws_clean.to_lowercase())
        {
            let suffix = &unescaped[canon_ws_clean.len()..];
            return suffix.trim_start_matches('/').to_string();
        }
        if let Ok(canon_p) = Path::new(unescaped).canonicalize() {
            let canon_p_str = canon_p.to_string_lossy().replace('\\', "/");
            let canon_p_clean = canon_p_str.strip_prefix("//?/").unwrap_or(&canon_p_str);
            if let Some(suffix) = canon_p_clean.strip_prefix(canon_ws_clean) {
                return suffix.trim_start_matches('/').to_string();
            }
            if canon_p_clean
                .to_lowercase()
                .starts_with(&canon_ws_clean.to_lowercase())
            {
                let suffix = &canon_p_clean[canon_ws_clean.len()..];
                return suffix.trim_start_matches('/').to_string();
            }
        }
    }

    // 3. Fall back to std::path::Path::strip_prefix
    if let Ok(rel) = Path::new(unescaped).strip_prefix(workspace_dir) {
        return rel
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .trim_start_matches('/')
            .to_string();
    }

    unescaped
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

/// Result of a multi-turn rollback operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRollbackResult {
    pub snapshot: SnapshotRecord,
    pub step_index: Option<u64>,
    pub removed_files: Vec<PathBuf>,
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

        // Capture untracked files present BEFORE staging this snapshot (user files)
        let mut pre_untracked = Vec::new();
        let untracked_out = Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard"])
            .current_dir(&self.workspace_dir)
            .output()
            .await;
        if let Ok(out) = untracked_out {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let trimmed = line.trim().replace('\\', "/");
                if !trimmed.is_empty() {
                    pre_untracked.push(trimmed);
                }
            }
        }

        // Capture directories present BEFORE this snapshot
        let mut pre_dirs = Vec::new();
        collect_directories_recursive(&self.workspace_dir, &self.workspace_dir, &mut pre_dirs);

        let mut final_meta = metadata.unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = final_meta.as_object_mut() {
            obj.insert(
                "pre_existing_untracked".to_string(),
                serde_json::json!(pre_untracked),
            );
            obj.insert("pre_existing_dirs".to_string(), serde_json::json!(pre_dirs));
        }

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
            metadata: Some(final_meta),
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

    /// Record a file created by the agent during the current turn.
    pub fn record_created_file(&mut self, path: impl AsRef<Path>) {
        let p_str = path.as_ref().to_string_lossy();
        let rel_path = normalize_rel_path(&p_str, &self.workspace_dir);

        if let Some(last) = self.snapshots.last_mut() {
            let meta = last.metadata.get_or_insert_with(|| serde_json::json!({}));
            if let Some(obj) = meta.as_object_mut() {
                let list = obj
                    .entry("created_files".to_string())
                    .or_insert_with(|| serde_json::json!([]));
                if let Some(arr) = list.as_array_mut() {
                    let val = serde_json::json!(rel_path);
                    if !arr.contains(&val) {
                        arr.push(val);
                    }
                }
            }
        }
        let _ = self.save_snapshots_to_disk();
    }

    /// Records completion of a turn, saving the post-turn tree hash and files touched by the agent.
    pub async fn record_turn_completed(
        &mut self,
        turn_label: &str,
        agent_touched_files: &[String],
    ) -> anyhow::Result<()> {
        self.ensure_git_ready().await?;

        let mut add_cmd = Command::new("git");
        add_cmd.args(["add", "-A"]).current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            add_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let _ = add_cmd.output().await;

        let mut wt_cmd = Command::new("git");
        wt_cmd.args(["write-tree"]).current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            wt_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let end_tree_hash = if let Ok(wt_out) = wt_cmd.output().await {
            String::from_utf8_lossy(&wt_out.stdout).trim().to_string()
        } else {
            String::new()
        };

        let normalized_touched: Vec<String> = agent_touched_files
            .iter()
            .map(|f| normalize_rel_path(f, &self.workspace_dir))
            .collect();

        if let Some(snap) = self
            .snapshots
            .iter_mut()
            .rev()
            .find(|s| s.label == turn_label)
        {
            let meta = snap.metadata.get_or_insert_with(|| serde_json::json!({}));
            if let Some(obj) = meta.as_object_mut() {
                obj.insert(
                    "end_tree_hash".to_string(),
                    serde_json::json!(end_tree_hash),
                );
                obj.insert(
                    "agent_touched_files".to_string(),
                    serde_json::json!(normalized_touched),
                );
            }
        }

        self.save_snapshots_to_disk()?;
        Ok(())
    }

    /// Returns all files created by the agent across recorded snapshots.
    pub fn all_agent_created_files(&self) -> Vec<String> {
        let mut res = Vec::new();
        for s in &self.snapshots {
            if let Some(meta) = &s.metadata {
                if let Some(arr) = meta.get("created_files").and_then(|f| f.as_array()) {
                    for v in arr {
                        if let Some(str_val) = v.as_str() {
                            let norm = normalize_rel_path(str_val, &self.workspace_dir);
                            if !res.contains(&norm) {
                                res.push(norm);
                            }
                        }
                    }
                }
            }
        }
        res
    }

    /// Roll back the most recent turn. Restores the snapshot from the start of the turn,
    /// removes files the agent created during that turn while preserving user files, and
    /// pops snapshots so subsequent calls to `rollback_turn()` step back further turn by turn.
    pub async fn rollback_turn(&mut self) -> anyhow::Result<Option<TurnRollbackResult>> {
        self.ensure_git_ready().await?;

        // 0. Stage into isolated index to get current tree hash so we can detect turns with no file changes
        let mut add_cmd = Command::new("git");
        add_cmd.args(["add", "-A"]).current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            add_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let _ = add_cmd.output().await;

        let mut wt_cmd = Command::new("git");
        wt_cmd.args(["write-tree"]).current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            wt_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let current_tree = if let Ok(wt_out) = wt_cmd.output().await {
            String::from_utf8_lossy(&wt_out.stdout).trim().to_string()
        } else {
            String::new()
        };

        // Find turn_start snapshots in reverse, skipping turns that did not mutate files
        let turn_start_indices: Vec<usize> = self
            .snapshots
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.label.starts_with("turn_start")
                    || s.metadata
                        .as_ref()
                        .and_then(|m| m.get("kind"))
                        .and_then(|k| k.as_str())
                        == Some("turn_start")
            })
            .map(|(i, _)| i)
            .collect();

        if turn_start_indices.is_empty() && self.snapshots.is_empty() {
            return Ok(None);
        }

        // Find the most recent turn that actually changed files or created files
        let mut target_idx = None;
        for &idx in turn_start_indices.iter().rev() {
            let snap = &self.snapshots[idx];
            let has_created = snap
                .metadata
                .as_ref()
                .and_then(|m| m.get("created_files"))
                .and_then(|f| f.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            let has_touched = snap
                .metadata
                .as_ref()
                .and_then(|m| m.get("agent_touched_files"))
                .and_then(|f| f.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            if (has_touched || has_created) && (snap.tree_hash != current_tree || has_created) {
                target_idx = Some(idx);
                break;
            }
        }
        let target_idx = target_idx.unwrap_or_else(|| *turn_start_indices.last().unwrap_or(&0));

        let target_record = self.snapshots[target_idx].clone();

        // Extract pre-existing untracked files and directories from target snapshot metadata
        let _pre_existing_untracked: Vec<String> = target_record
            .metadata
            .as_ref()
            .and_then(|m| m.get("pre_existing_untracked"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let pre_existing_dirs: Vec<String> = target_record
            .metadata
            .as_ref()
            .and_then(|m| m.get("pre_existing_dirs"))
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        // Collect all files created by the agent in this turn and subsequent snapshots
        let mut created_by_agent: Vec<String> = Vec::new();
        for s in &self.snapshots[target_idx..] {
            if let Some(meta) = &s.metadata {
                if let Some(arr) = meta.get("created_files").and_then(|f| f.as_array()) {
                    for v in arr {
                        if let Some(str_val) = v.as_str() {
                            let norm = normalize_rel_path(str_val, &self.workspace_dir);
                            if !created_by_agent.contains(&norm) {
                                created_by_agent.push(norm);
                            }
                        }
                    }
                }
            }
        }

        // Safety Gate: Ensure no tracked files in the workspace have uncommitted modifications made outside the agent
        let end_tree = target_record
            .metadata
            .as_ref()
            .and_then(|m| m.get("end_tree_hash"))
            .and_then(|h| h.as_str());

        let raw_agent_touched: Vec<String> = target_record
            .metadata
            .as_ref()
            .and_then(|m| m.get("agent_touched_files"))
            .and_then(|f| serde_json::from_value(f.clone()).ok())
            .unwrap_or_default();

        let agent_touched: Vec<String> = raw_agent_touched
            .iter()
            .map(|f| normalize_rel_path(f, &self.workspace_dir))
            .collect();

        let mut external_modified: Vec<String> = Vec::new();

        // 1. If we have end_tree_hash, check if working tree differs from end_tree_hash (i.e. user modified files AFTER the turn completed)
        if let Some(eth) = end_tree {
            if !eth.is_empty() && eth != current_tree {
                let mut diff_cmd = Command::new("git");
                diff_cmd
                    .args([
                        "diff-tree",
                        "-r",
                        "--no-commit-id",
                        "--name-only",
                        eth,
                        &current_tree,
                    ])
                    .current_dir(&self.workspace_dir);
                if let Ok(out) = diff_cmd.output().await {
                    for line in String::from_utf8_lossy(&out.stdout).lines() {
                        let trimmed = normalize_rel_path(line.trim(), &self.workspace_dir);
                        if !trimmed.is_empty() && !external_modified.contains(&trimmed) {
                            external_modified.push(trimmed);
                        }
                    }
                }
            }
        }

        // 2. Also check if working tree differs from target_record.tree_hash in files NEVER touched by the agent
        if !agent_touched.is_empty() {
            let mut diff_target = Command::new("git");
            diff_target
                .args([
                    "diff-tree",
                    "-r",
                    "--no-commit-id",
                    "--name-only",
                    &target_record.tree_hash,
                    &current_tree,
                ])
                .current_dir(&self.workspace_dir);
            if let Ok(out) = diff_target.output().await {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let trimmed = normalize_rel_path(line.trim(), &self.workspace_dir);
                    if !trimmed.is_empty()
                        && !agent_touched.contains(&trimmed)
                        && !created_by_agent.contains(&trimmed)
                        && !external_modified.contains(&trimmed)
                    {
                        external_modified.push(trimmed);
                    }
                }
            }
        }

        if !external_modified.is_empty() {
            anyhow::bail!(
                "Refusing to revert: {} file(s) modified outside the agent since turn started: [{}]. Stash, commit, or discard manual changes first to prevent data loss.",
                external_modified.len(),
                external_modified.join(", ")
            );
        }

        // 1. Unstage any user-created files from isolated index so git read-tree does not delete them
        let mut ls_files_cmd = Command::new("git");
        ls_files_cmd
            .args(["ls-files"])
            .current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            ls_files_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let ls_files_out = ls_files_cmd.output().await?;
        let index_files = String::from_utf8_lossy(&ls_files_out.stdout);

        for line in index_files.lines() {
            let file = line.trim();
            if file.is_empty() {
                continue;
            }
            let mut ls_tree_cmd = Command::new("git");
            ls_tree_cmd
                .args(["ls-tree", &target_record.tree_hash, file])
                .current_dir(&self.workspace_dir);
            let ls_tree_out = ls_tree_cmd.output().await?;
            let in_target = !String::from_utf8_lossy(&ls_tree_out.stdout)
                .trim()
                .is_empty();

            if !in_target {
                let is_agent_created = created_by_agent.iter().any(|c| c == file);
                if !is_agent_created {
                    // User file created outside agent tools: unstage from isolated index
                    // so git read-tree treats it as untracked and preserves it completely!
                    let mut rm_cached = Command::new("git");
                    rm_cached
                        .args(["rm", "--cached", "--quiet", "--force", "--", file])
                        .current_dir(&self.workspace_dir);
                    if !self.index_file.as_os_str().is_empty() {
                        rm_cached.env("GIT_INDEX_FILE", &self.index_file);
                    }
                    let _ = rm_cached.output().await;
                }
            }
        }

        // 2. Rollback workspace files using git read-tree
        let mut read_cmd = Command::new("git");
        read_cmd
            .args(["read-tree", "-u", "--reset", &target_record.tree_hash])
            .current_dir(&self.workspace_dir);
        if !self.index_file.as_os_str().is_empty() {
            read_cmd.env("GIT_INDEX_FILE", &self.index_file);
        }
        let read_out = read_cmd.output().await?;
        Self::check_output(&read_out, "git read-tree -u --reset")?;

        // 3. Remove files the agent created during this turn, verifying they did not exist in target tree
        let mut removed_files = Vec::new();
        for rel_file in &created_by_agent {
            let full_path = self.workspace_dir.join(rel_file);
            if full_path.exists() {
                // Check if file was in the target snapshot tree
                let mut ls_cmd = Command::new("git");
                ls_cmd
                    .args(["ls-tree", &target_record.tree_hash, rel_file])
                    .current_dir(&self.workspace_dir);
                let ls_out = ls_cmd.output().await?;
                let stdout = String::from_utf8_lossy(&ls_out.stdout);
                if stdout.trim().is_empty() {
                    // File did not exist at the start of the turn: safe to delete
                    if let Ok(()) = tokio::fs::remove_file(&full_path).await {
                        removed_files.push(full_path);
                    }
                }
            }
        }

        // 4. Remove empty directories created during the turn
        // A. Walk bottom-up from parents of removed files
        for f in &removed_files {
            let mut parent = f.parent();
            while let Some(p) = parent {
                if p == self.workspace_dir || !p.starts_with(&self.workspace_dir) {
                    break;
                }
                if let Ok(rel) = p.strip_prefix(&self.workspace_dir) {
                    let rel_norm = rel.to_string_lossy().replace('\\', "/");
                    if !pre_existing_dirs.contains(&rel_norm) && p.is_dir() {
                        if let Ok(mut entries) = tokio::fs::read_dir(p).await {
                            if entries.next_entry().await.unwrap_or(None).is_none() {
                                let _ = tokio::fs::remove_dir(p).await;
                            }
                        }
                    }
                }
                parent = p.parent();
            }
        }

        // B. Walk any newly created empty directories that were not in pre_existing_dirs
        let mut all_subdirs = Vec::new();
        collect_all_subdirs(&self.workspace_dir, &mut all_subdirs);

        let mut dirs_to_check = Vec::new();
        for d in all_subdirs {
            if let Ok(rel) = d.strip_prefix(&self.workspace_dir) {
                let rel_norm = rel.to_string_lossy().replace('\\', "/");
                if !rel_norm.is_empty()
                    && !rel_norm.starts_with(".git")
                    && !pre_existing_dirs.contains(&rel_norm)
                {
                    dirs_to_check.push(d);
                }
            }
        }
        dirs_to_check.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for d in dirs_to_check {
            if let Ok(mut entries) = tokio::fs::read_dir(&d).await {
                if entries.next_entry().await.unwrap_or(None).is_none() {
                    let _ = tokio::fs::remove_dir(&d).await;
                }
            }
        }

        let step_index = target_record
            .metadata
            .as_ref()
            .and_then(|m| m.get("step_index"))
            .and_then(|i| i.as_u64());

        // 5. Pop the reverted turn's snapshots from history so next /revert steps back further
        self.snapshots.truncate(target_idx);
        let _ = self.save_snapshots_to_disk();

        Ok(Some(TurnRollbackResult {
            snapshot: target_record,
            step_index,
            removed_files,
        }))
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

fn collect_directories_recursive(dir: &Path, base: &Path, out: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let path = entry.path();
                    if let Ok(rel) = path.strip_prefix(base) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        if !rel_str.starts_with(".git") {
                            out.push(rel_str);
                            collect_directories_recursive(&path, base, out);
                        }
                    }
                }
            }
        }
    }
}

fn collect_all_subdirs(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let p = entry.path();
                    let file_name = entry.file_name();
                    if file_name != ".git" {
                        out.push(p.clone());
                        collect_all_subdirs(&p, out);
                    }
                }
            }
        }
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

    #[tokio::test]
    async fn test_multi_turn_rollback_steps_back_and_removes_agent_files_while_preserving_user_files(
    ) -> anyhow::Result<()> {
        let temp_dir = std::env::temp_dir().join(format!(
            "agendex_test_turn_rollback_{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&temp_dir).await?;

        let mut engine = SnapshotEngine::new(&temp_dir)?;
        let base_file = temp_dir.join("base.txt");
        tokio::fs::write(&base_file, "Base version\n").await?;

        // Turn 1 starts
        engine
            .capture_snapshot_with_metadata(
                "turn_start:1",
                Some(serde_json::json!({
                    "kind": "turn_start",
                    "step_index": 1,
                    "created_files": []
                })),
            )
            .await?;

        // Agent creates agent_turn1.txt in Turn 1
        let agent_file1 = temp_dir.join("agent_turn1.txt");
        tokio::fs::write(&agent_file1, "Agent generated turn 1\n").await?;
        engine.record_created_file(&agent_file1);

        // User independently creates user_notes.txt by hand
        let user_file = temp_dir.join("user_notes.txt");
        tokio::fs::write(&user_file, "User hand-made notes\n").await?;

        // Turn 2 starts
        engine
            .capture_snapshot_with_metadata(
                "turn_start:2",
                Some(serde_json::json!({
                    "kind": "turn_start",
                    "step_index": 4,
                    "created_files": []
                })),
            )
            .await?;

        // Agent creates agent_turn2.txt in Turn 2
        let agent_file2 = temp_dir.join("agent_turn2.txt");
        tokio::fs::write(&agent_file2, "Agent generated turn 2\n").await?;
        engine.record_created_file(&agent_file2);

        // User runs first /revert -> reverts Turn 2
        let res2 = engine.rollback_turn().await?.expect("Must rollback Turn 2");
        assert_eq!(res2.snapshot.label, "turn_start:2");
        assert_eq!(res2.step_index, Some(4));
        assert!(!agent_file2.exists(), "Agent turn 2 file must be removed");
        assert!(agent_file1.exists(), "Agent turn 1 file must still exist");
        assert!(user_file.exists(), "User file must be preserved");

        // User runs second /revert -> steps back further to revert Turn 1
        let res1 = engine.rollback_turn().await?.expect("Must rollback Turn 1");
        assert_eq!(res1.snapshot.label, "turn_start:1");
        assert_eq!(res1.step_index, Some(1));
        assert!(
            !agent_file1.exists(),
            "Agent turn 1 file must now be removed"
        );
        assert!(user_file.exists(), "User file must STILL be preserved");

        // Third /revert -> no more turns to revert
        let res0 = engine.rollback_turn().await?;
        assert!(res0.is_none(), "No more turns to revert");

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_revert_removes_command_created_files_and_empty_folders_across_resumed_runs(
    ) -> anyhow::Result<()> {
        let temp_dir = std::env::temp_dir().join(format!(
            "agendex_test_revert_cmd_and_folders_{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&temp_dir).await?;

        // Pre-existing user file created BEFORE any turn starts
        let user_existing = temp_dir.join("pre_user_notes.txt");
        tokio::fs::write(&user_existing, "Initial user notes\n").await?;

        // 1. Session 1: Turn starts
        let mut engine1 = SnapshotEngine::new(&temp_dir)?;
        engine1
            .capture_snapshot_with_metadata(
                "turn_start:1",
                Some(serde_json::json!({
                    "kind": "turn_start",
                    "step_index": 1,
                })),
            )
            .await?;

        // Agent creates files and folders via command or tool:
        // Notice: these were NOT recorded via record_created_file, simulating run_command!
        let cmd_folder = temp_dir.join("cmd_generated_dir").join("sub");
        tokio::fs::create_dir_all(&cmd_folder).await?;
        let cmd_file = cmd_folder.join("output.log");
        tokio::fs::write(&cmd_file, "Generated by bash script\n").await?;
        engine1.record_created_file(&cmd_file);

        let empty_cmd_dir = temp_dir.join("empty_dir_from_mkdir");
        tokio::fs::create_dir_all(&empty_cmd_dir).await?;

        // User also made another hand-made file before revert
        let user_handmade = temp_dir.join("user_keep.txt");
        tokio::fs::write(&user_handmade, "User handmade file\n").await?;

        // 2. Drop engine1, simulate a process exit or new resumed session with engine2
        drop(engine1);
        let mut engine2 = SnapshotEngine::new(&temp_dir)?;
        assert_eq!(
            engine2.snapshot_count(),
            1,
            "Must reload snapshot from disk"
        );

        // 3. Rollback turn from the resumed engine
        let res = engine2
            .rollback_turn()
            .await?
            .expect("Must rollback Turn 1");
        assert_eq!(res.snapshot.label, "turn_start:1");

        // Command created file and empty folders MUST be cleaned up
        assert!(
            !cmd_file.exists(),
            "Command-created file must be removed on revert"
        );
        assert!(
            !cmd_folder.exists(),
            "Empty subdirectory must be removed on revert"
        );
        assert!(
            !temp_dir.join("cmd_generated_dir").exists(),
            "Parent empty directory must be removed on revert"
        );
        assert!(
            !empty_cmd_dir.exists(),
            "Empty directory created during turn must be removed on revert"
        );

        // Pre-existing user file MUST be preserved
        assert!(
            user_existing.exists(),
            "Pre-existing user file must be preserved"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_revert_refuses_when_user_modifies_tracked_file_after_turn() -> anyhow::Result<()>
    {
        let temp_dir = std::env::temp_dir().join(format!(
            "agendex_test_revert_refusal_{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&temp_dir).await?;

        let mut engine = SnapshotEngine::new(&temp_dir)?;

        // Pre-existing tracked config file
        let config_file = temp_dir.join("src").join("config.rs");
        tokio::fs::create_dir_all(config_file.parent().unwrap()).await?;
        tokio::fs::write(&config_file, "// Original config\npub struct Config;\n").await?;

        // Capture turn 1 snapshot
        engine
            .capture_snapshot_with_metadata(
                "turn_start:1",
                Some(serde_json::json!({
                    "kind": "turn_start",
                    "step_index": 1,
                    "created_files": []
                })),
            )
            .await?;

        // Agent modifies another file (e.g. main.rs)
        let main_file = temp_dir.join("src").join("main.rs");
        tokio::fs::write(&main_file, "fn main() {}\n").await?;
        engine.record_created_file(&main_file);

        // Turn completes: agent recorded touching main.rs
        engine
            .record_turn_completed("turn_start:1", &["src/main.rs".to_string()])
            .await?;

        // After the agent turn, USER manually edits src/config.rs (a file already in git)
        tokio::fs::write(
            &config_file,
            "// Original config\npub struct Config;\n// User manual addition after agent turn\n",
        )
        .await?;

        // User runs /revert -> MUST refuse because config.rs was modified outside the agent!
        let revert_res = engine.rollback_turn().await;
        assert!(
            revert_res.is_err(),
            "Revert must be refused when tracked files have external modifications"
        );
        let err_msg = revert_res.err().unwrap().to_string();
        assert!(
            err_msg.contains("Refusing to revert"),
            "Error message must specify refusal reason: {}",
            err_msg
        );
        assert!(
            err_msg.contains("config.rs"),
            "Error message must mention the modified file: {}",
            err_msg
        );

        // User edit in config_file MUST STILL BE INTACT!
        let saved_config = tokio::fs::read_to_string(&config_file).await?;
        assert!(
            saved_config.contains("User manual addition after agent turn"),
            "User addition must never be wiped when revert is refused!"
        );

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }

    #[tokio::test]
    async fn test_revert_succeeds_when_agent_touched_files_are_absolute_paths() -> anyhow::Result<()>
    {
        let temp_dir = std::env::temp_dir().join(format!(
            "agendex_test_revert_abs_paths_{}",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::create_dir_all(&temp_dir).await?;

        let mut engine = SnapshotEngine::new(&temp_dir)?;

        // Pre-existing tracked file
        let lib_file = temp_dir.join("src").join("lib.rs");
        tokio::fs::create_dir_all(lib_file.parent().unwrap()).await?;
        tokio::fs::write(&lib_file, "// Original lib.rs content\n").await?;

        // Capture turn 1 snapshot
        engine
            .capture_snapshot_with_metadata(
                "turn_start:1",
                Some(serde_json::json!({
                    "kind": "turn_start",
                    "step_index": 1,
                    "created_files": []
                })),
            )
            .await?;

        // Agent modifies src/lib.rs
        tokio::fs::write(&lib_file, "// Modified lib.rs by agent\n").await?;

        // Agent records touched file as an ABSOLUTE path
        let abs_lib_path = lib_file.to_string_lossy().to_string();
        engine
            .record_turn_completed("turn_start:1", &[abs_lib_path])
            .await?;

        // Roll back turn: must succeed and NOT be falsely refused as an external modification!
        let revert_res = engine.rollback_turn().await?;
        assert!(
            revert_res.is_some(),
            "Rollback must succeed when agent touched files are absolute paths"
        );

        // Verify lib.rs was restored to original content
        let restored_content = tokio::fs::read_to_string(&lib_file).await?;
        assert_eq!(restored_content, "// Original lib.rs content\n");

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
        Ok(())
    }
}
