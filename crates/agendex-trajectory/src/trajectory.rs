use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::telemetry::{format_telemetry, SessionUsage, TokenUsage};

/// Maximum characters allowed in compact trajectory log before truncation triggers.
pub const DEFAULT_COMPACT_CHAR_LIMIT: usize = 1000;

/// Represents a single discrete turn or step within the conversation trajectory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryStep {
    pub step_index: u64,
    pub source: String,
    #[serde(rename = "type")]
    pub step_type: String,
    pub status: String,
    pub created_at: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_fields: Option<Vec<String>>,
}

/// Rich input parameters for recording detailed trajectory turns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryStepInput {
    pub step_type: String,
    pub source: String,
    pub status: String,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Option<serde_json::Value>,
    pub media: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
}

/// Checkpoint entry mapping a conversation turn to a workspace snapshot hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckpointRecord {
    pub id: i64,
    pub step_index: u64,
    pub label: String,
    pub tree_hash: Option<String>,
    pub created_at: String,
}

/// Dual-tier trajectory logger and SQLite turn persistence engine.
pub struct TrajectoryLogger {
    log_dir: Option<PathBuf>,
    compact_path: Option<PathBuf>,
    full_path: Option<PathBuf>,
    compact_char_limit: usize,
    step_counter: u64,
    in_memory_steps: Vec<TrajectoryStep>,
    db: Arc<Mutex<Connection>>,
    session_usage: SessionUsage,
    last_turn_usage: Option<TokenUsage>,
}

impl TrajectoryLogger {
    /// Initialize a new dual-tier trajectory logger targeting the specified directory.
    pub fn new(log_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let log_dir = log_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&log_dir)?;

        let compact_path = log_dir.join("trajectory.jsonl");
        let full_path = log_dir.join("trajectory_full.jsonl");
        let db_path = log_dir.join("trajectory.db");

        let conn = Connection::open(&db_path)?;
        Self::init_db_schema(&conn)?;

        // Recover existing step count from SQLite or files
        let step_counter: u64 = conn
            .query_row(
                "SELECT COALESCE(MAX(step_index), 0) FROM trajectory_steps",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let mut logger = Self {
            log_dir: Some(log_dir),
            compact_path: Some(compact_path),
            full_path: Some(full_path),
            compact_char_limit: DEFAULT_COMPACT_CHAR_LIMIT,
            step_counter,
            in_memory_steps: Vec::new(),
            db: Arc::new(Mutex::new(conn)),
            session_usage: SessionUsage::default(),
            last_turn_usage: None,
        };

        logger.reload_steps_from_db()?;
        Ok(logger)
    }

    /// Initialize an in-memory trajectory logger (ideal for unit testing and ephemeral sandboxes).
    pub fn new_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_db_schema(&conn)?;

        Ok(Self {
            log_dir: None,
            compact_path: None,
            full_path: None,
            compact_char_limit: DEFAULT_COMPACT_CHAR_LIMIT,
            step_counter: 0,
            in_memory_steps: Vec::new(),
            db: Arc::new(Mutex::new(conn)),
            session_usage: SessionUsage::default(),
            last_turn_usage: None,
        })
    }

    /// Configure custom character threshold for compact trajectory truncation.
    pub fn set_compact_char_limit(&mut self, limit: usize) {
        self.compact_char_limit = limit;
    }

    /// Initialize SQLite database schema for trajectory turns and checkpoints.
    fn init_db_schema(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS trajectory_steps (
                step_index INTEGER PRIMARY KEY,
                step_type TEXT NOT NULL,
                source TEXT NOT NULL,
                status TEXT NOT NULL,
                content TEXT NOT NULL,
                thinking TEXT,
                tool_calls TEXT,
                media TEXT,
                metadata TEXT,
                created_at TEXT NOT NULL,
                is_compacted INTEGER DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS trajectory_checkpoints (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                step_index INTEGER NOT NULL,
                label TEXT NOT NULL,
                tree_hash TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS trajectory_token_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                step_index INTEGER,
                prompt_tokens INTEGER NOT NULL,
                completion_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                estimated_cost REAL NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_steps_created_at ON trajectory_steps(created_at);
            CREATE INDEX IF NOT EXISTS idx_checkpoints_step ON trajectory_checkpoints(step_index);
            CREATE INDEX IF NOT EXISTS idx_token_usage_created ON trajectory_token_usage(created_at);
            "#,
        )?;
        Ok(())
    }

    /// Reload step history from the SQLite database.
    fn reload_steps_from_db(&mut self) -> anyhow::Result<()> {
        let conn = self
            .db
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT step_index, step_type, source, status, content, thinking, tool_calls, media, metadata, created_at
             FROM trajectory_steps ORDER BY step_index ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            let step_index: u64 = row.get(0)?;
            let step_type: String = row.get(1)?;
            let source: String = row.get(2)?;
            let status: String = row.get(3)?;
            let content: String = row.get(4)?;
            let thinking: Option<String> = row.get(5)?;
            let tool_calls_raw: Option<String> = row.get(6)?;
            let media_raw: Option<String> = row.get(7)?;
            let metadata_raw: Option<String> = row.get(8)?;
            let created_at: String = row.get(9)?;

            let tool_calls = tool_calls_raw.and_then(|s| serde_json::from_str(&s).ok());
            let media = media_raw.and_then(|s| serde_json::from_str(&s).ok());
            let metadata = metadata_raw.and_then(|s| serde_json::from_str(&s).ok());

            Ok(TrajectoryStep {
                step_index,
                source,
                step_type,
                status,
                created_at,
                content,
                thinking,
                tool_calls,
                media,
                metadata,
                truncated_fields: None,
            })
        })?;

        self.in_memory_steps.clear();
        for row in rows {
            self.in_memory_steps.push(row?);
        }

        // Reload cumulative token and cost usage
        if let Ok(mut stmt_usage) = conn.prepare(
            "SELECT COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(total_tokens), 0), COALESCE(SUM(estimated_cost), 0.0), COUNT(*)
             FROM trajectory_token_usage",
        ) {
            if let Ok(usage) = stmt_usage.query_row([], |row| {
                Ok(SessionUsage {
                    total_prompt_tokens: row.get::<_, i64>(0)? as usize,
                    total_completion_tokens: row.get::<_, i64>(1)? as usize,
                    total_tokens: row.get::<_, i64>(2)? as usize,
                    total_cost: row.get::<_, f64>(3)?,
                    turn_count: row.get::<_, i64>(4)? as usize,
                })
            }) {
                self.session_usage = usage;
            }
        }

        Ok(())
    }

    /// Records token consumption and cost for a turn to SQLite and in-memory session totals.
    pub fn record_token_usage(
        &mut self,
        step_index: Option<u64>,
        prompt_tokens: usize,
        completion_tokens: usize,
        estimated_cost: f64,
    ) -> anyhow::Result<TokenUsage> {
        let turn = TokenUsage::new(prompt_tokens, completion_tokens, estimated_cost);
        let created_at = Utc::now().to_rfc3339();

        {
            let conn = self
                .db
                .lock()
                .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
            conn.execute(
                "INSERT INTO trajectory_token_usage (
                    step_index, prompt_tokens, completion_tokens, total_tokens, estimated_cost, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    step_index.map(|s| s as i64),
                    turn.prompt_tokens as i64,
                    turn.completion_tokens as i64,
                    turn.total_tokens as i64,
                    turn.estimated_cost,
                    created_at,
                ],
            )?;
        }

        self.session_usage.record_turn(&turn);
        self.last_turn_usage = Some(turn.clone());
        Ok(turn)
    }

    /// Returns cumulative session token usage and estimated cost.
    pub fn session_usage(&self) -> SessionUsage {
        self.session_usage.clone()
    }

    /// Returns the token usage breakdown from the most recent turn.
    pub fn last_turn_usage(&self) -> Option<TokenUsage> {
        self.last_turn_usage.clone()
    }

    /// Formats the most recent turn and cumulative session usage into standard telemetry string.
    pub fn format_last_telemetry(&self) -> Option<String> {
        self.last_turn_usage
            .as_ref()
            .map(|turn| format_telemetry(turn, &self.session_usage))
    }

    /// Primary async API compatible with `agendex-core`: logs a step to dual trajectory logs and SQLite.
    pub async fn log_step(
        &mut self,
        step_type: &str,
        source: &str,
        content: &str,
        metadata: serde_json::Value,
    ) -> anyhow::Result<TrajectoryStep> {
        self.log_step_detailed(TrajectoryStepInput {
            step_type: step_type.to_string(),
            source: source.to_string(),
            status: "DONE".to_string(),
            content: content.to_string(),
            thinking: None,
            tool_calls: None,
            media: None,
            metadata: Some(metadata),
        })
        .await
    }

    /// Log a detailed trajectory turn with complete thinking, tool calls, media, and status.
    pub async fn log_step_detailed(
        &mut self,
        input: TrajectoryStepInput,
    ) -> anyhow::Result<TrajectoryStep> {
        self.step_counter += 1;
        let step_index = self.step_counter;
        let created_at = Utc::now().to_rfc3339();

        // 1. Full representation (untruncated audit log)
        let full_step = TrajectoryStep {
            step_index,
            source: input.source.clone(),
            step_type: input.step_type.clone(),
            status: input.status.clone(),
            created_at: created_at.clone(),
            content: input.content.clone(),
            thinking: input.thinking.clone(),
            tool_calls: input.tool_calls.clone(),
            media: input.media.clone(),
            metadata: input.metadata.clone(),
            truncated_fields: None,
        };

        // 2. Compact representation (truncated if fields exceed compact threshold)
        let mut truncated_fields = Vec::new();
        let mut compact_content = input.content.clone();
        if compact_content.len() > self.compact_char_limit {
            let orig_len = compact_content.len();
            let truncated_part: String = compact_content
                .chars()
                .take(self.compact_char_limit)
                .collect();
            compact_content = format!(
                "{}... [truncated {} chars; see trajectory_full.jsonl]",
                truncated_part,
                orig_len - self.compact_char_limit
            );
            truncated_fields.push("content".to_string());
        }

        let mut compact_thinking = input.thinking.clone();
        if let Some(ref th) = input.thinking {
            if th.len() > self.compact_char_limit {
                let orig_len = th.len();
                let truncated_part: String = th.chars().take(self.compact_char_limit).collect();
                compact_thinking = Some(format!(
                    "{}... [truncated {} chars; see trajectory_full.jsonl]",
                    truncated_part,
                    orig_len - self.compact_char_limit
                ));
                truncated_fields.push("thinking".to_string());
            }
        }

        let compact_step = TrajectoryStep {
            step_index,
            source: input.source,
            step_type: input.step_type,
            status: input.status,
            created_at,
            content: compact_content,
            thinking: compact_thinking,
            tool_calls: input.tool_calls,
            media: input.media,
            metadata: input.metadata,
            truncated_fields: if truncated_fields.is_empty() {
                None
            } else {
                Some(truncated_fields)
            },
        };

        // 3. Persist to SQLite
        {
            let conn = self
                .db
                .lock()
                .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
            let tool_calls_json = full_step.tool_calls.as_ref().map(|v| v.to_string());
            let media_json = full_step.media.as_ref().map(|v| v.to_string());
            let metadata_json = full_step.metadata.as_ref().map(|v| v.to_string());

            conn.execute(
                "INSERT INTO trajectory_steps (
                    step_index, step_type, source, status, content, thinking, tool_calls, media, metadata, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    full_step.step_index as i64,
                    full_step.step_type,
                    full_step.source,
                    full_step.status,
                    full_step.content,
                    full_step.thinking,
                    tool_calls_json,
                    media_json,
                    metadata_json,
                    full_step.created_at,
                ],
            )?;
        }

        // 4. Append to JSONL files if on-disk paths are configured
        if let Some(ref c_path) = self.compact_path {
            let mut file = OpenOptions::new().create(true).append(true).open(c_path)?;
            let compact_json = serde_json::to_string(&compact_step)?;
            writeln!(file, "{}", compact_json)?;
        }

        if let Some(ref f_path) = self.full_path {
            let mut file = OpenOptions::new().create(true).append(true).open(f_path)?;
            let full_json = serde_json::to_string(&full_step)?;
            writeln!(file, "{}", full_json)?;
        }

        // 5. Update in-memory cache
        self.in_memory_steps.push(full_step.clone());

        Ok(full_step)
    }

    /// Record a checkpoint associated with the current trajectory turn and an optional git tree hash.
    pub fn save_checkpoint(&mut self, label: &str, tree_hash: Option<&str>) -> anyhow::Result<i64> {
        let conn = self
            .db
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
        let created_at = Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO trajectory_checkpoints (step_index, label, tree_hash, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![self.step_counter as i64, label, tree_hash, created_at],
        )?;

        let id = conn.last_insert_rowid();
        Ok(id)
    }

    /// Retrieve all recorded checkpoints.
    pub fn get_checkpoints(&self) -> anyhow::Result<Vec<CheckpointRecord>> {
        let conn = self
            .db
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, step_index, label, tree_hash, created_at
             FROM trajectory_checkpoints ORDER BY id ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(CheckpointRecord {
                id: row.get(0)?,
                step_index: row.get::<_, i64>(1)? as u64,
                label: row.get(2)?,
                tree_hash: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    /// Read an untruncated step by step_index directly from SQLite.
    pub fn read_step_full(&self, step_index: u64) -> anyhow::Result<Option<TrajectoryStep>> {
        let conn = self
            .db
            .lock()
            .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT step_index, step_type, source, status, content, thinking, tool_calls, media, metadata, created_at
             FROM trajectory_steps WHERE step_index = ?1",
        )?;

        let mut rows = stmt.query_map(params![step_index as i64], |row| {
            let tool_calls_raw: Option<String> = row.get(6)?;
            let media_raw: Option<String> = row.get(7)?;
            let metadata_raw: Option<String> = row.get(8)?;

            Ok(TrajectoryStep {
                step_index: row.get::<_, i64>(0)? as u64,
                step_type: row.get(1)?,
                source: row.get(2)?,
                status: row.get(3)?,
                content: row.get(4)?,
                thinking: row.get(5)?,
                tool_calls: tool_calls_raw.and_then(|s| serde_json::from_str(&s).ok()),
                media: media_raw.and_then(|s| serde_json::from_str(&s).ok()),
                metadata: metadata_raw.and_then(|s| serde_json::from_str(&s).ok()),
                created_at: row.get(9)?,
                truncated_fields: None,
            })
        })?;

        if let Some(step) = rows.next() {
            Ok(Some(step?))
        } else {
            Ok(None)
        }
    }

    /// Truncate trajectory history after `step_index` (used by /revert time-travel).
    pub fn truncate_after(&mut self, target_step_index: u64) -> anyhow::Result<()> {
        // 1. Truncate SQLite rows
        {
            let conn = self
                .db
                .lock()
                .map_err(|e| anyhow::anyhow!("DB lock error: {}", e))?;
            conn.execute(
                "DELETE FROM trajectory_steps WHERE step_index > ?1",
                params![target_step_index as i64],
            )?;
            conn.execute(
                "DELETE FROM trajectory_checkpoints WHERE step_index > ?1",
                params![target_step_index as i64],
            )?;
        }

        // 2. Truncate in-memory steps
        self.in_memory_steps
            .retain(|s| s.step_index <= target_step_index);
        self.step_counter = target_step_index;

        // 3. Rewrite JSONL logs to maintain 1:1 parity
        if let Some(ref c_path) = self.compact_path {
            Self::rewrite_jsonl(c_path, &self.in_memory_steps, self.compact_char_limit, true)?;
        }
        if let Some(ref f_path) = self.full_path {
            Self::rewrite_jsonl(
                f_path,
                &self.in_memory_steps,
                self.compact_char_limit,
                false,
            )?;
        }

        Ok(())
    }

    /// Helper to rewrite a JSONL trajectory file during truncation.
    fn rewrite_jsonl(
        path: &Path,
        steps: &[TrajectoryStep],
        compact_char_limit: usize,
        apply_truncation: bool,
    ) -> anyhow::Result<()> {
        let mut file = File::create(path)?;
        for step in steps {
            let step_to_write = if apply_truncation {
                let mut truncated_fields = Vec::new();
                let mut content = step.content.clone();
                if content.len() > compact_char_limit {
                    let orig = content.len();
                    let part: String = content.chars().take(compact_char_limit).collect();
                    content = format!(
                        "{}... [truncated {} chars; see trajectory_full.jsonl]",
                        part,
                        orig - compact_char_limit
                    );
                    truncated_fields.push("content".to_string());
                }

                let mut thinking = step.thinking.clone();
                if let Some(ref th) = step.thinking {
                    if th.len() > compact_char_limit {
                        let orig = th.len();
                        let part: String = th.chars().take(compact_char_limit).collect();
                        thinking = Some(format!(
                            "{}... [truncated {} chars; see trajectory_full.jsonl]",
                            part,
                            orig - compact_char_limit
                        ));
                        truncated_fields.push("thinking".to_string());
                    }
                }

                TrajectoryStep {
                    step_index: step.step_index,
                    source: step.source.clone(),
                    step_type: step.step_type.clone(),
                    status: step.status.clone(),
                    created_at: step.created_at.clone(),
                    content,
                    thinking,
                    tool_calls: step.tool_calls.clone(),
                    media: step.media.clone(),
                    metadata: step.metadata.clone(),
                    truncated_fields: if truncated_fields.is_empty() {
                        None
                    } else {
                        Some(truncated_fields)
                    },
                }
            } else {
                let mut full = step.clone();
                full.truncated_fields = None;
                full
            };

            let line = serde_json::to_string(&step_to_write)?;
            writeln!(file, "{}", line)?;
        }
        Ok(())
    }

    /// Get all cached in-memory trajectory steps.
    pub fn get_steps(&self) -> &[TrajectoryStep] {
        &self.in_memory_steps
    }

    /// Return total step count recorded.
    pub fn step_count(&self) -> usize {
        self.in_memory_steps.len()
    }

    /// Return log directory path if configured.
    pub fn log_dir(&self) -> Option<&Path> {
        self.log_dir.as_deref()
    }

    /// Return path to compact trajectory.jsonl.
    pub fn compact_path(&self) -> Option<&Path> {
        self.compact_path.as_deref()
    }

    /// Return path to untruncated trajectory_full.jsonl.
    pub fn full_path(&self) -> Option<&Path> {
        self.full_path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[tokio::test]
    async fn test_dual_tier_logging_and_truncation() -> anyhow::Result<()> {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_test_traj_{}", uuid::Uuid::new_v4()));
        let mut logger = TrajectoryLogger::new(&temp_dir)?;
        logger.set_compact_char_limit(50);

        let short_content = "Short message";
        let long_content = "A".repeat(150);

        let s1 = logger
            .log_step(
                "USER_INPUT",
                "USER",
                short_content,
                serde_json::json!({"turn": 1}),
            )
            .await?;
        assert_eq!(s1.step_index, 1);
        assert_eq!(s1.content, short_content);

        let s2 = logger
            .log_step(
                "TOOL_RESULT",
                "TOOL",
                &long_content,
                serde_json::json!({"tool": "read_file"}),
            )
            .await?;
        assert_eq!(s2.step_index, 2);
        assert_eq!(s2.content, long_content);

        // Verify SQLite step count
        assert_eq!(logger.step_count(), 2);

        // Verify compact jsonl
        let compact_file = temp_dir.join("trajectory.jsonl");
        let compact_reader = BufReader::new(File::open(&compact_file)?);
        let compact_lines: Vec<String> = compact_reader.lines().collect::<Result<_, _>>()?;
        assert_eq!(compact_lines.len(), 2);

        let step1_compact: TrajectoryStep = serde_json::from_str(&compact_lines[0])?;
        assert_eq!(step1_compact.content, short_content);
        assert!(step1_compact.truncated_fields.is_none());

        let step2_compact: TrajectoryStep = serde_json::from_str(&compact_lines[1])?;
        assert!(step2_compact
            .content
            .contains("[truncated 100 chars; see trajectory_full.jsonl]"));
        assert_eq!(
            step2_compact.truncated_fields,
            Some(vec!["content".to_string()])
        );

        // Verify full jsonl
        let full_file = temp_dir.join("trajectory_full.jsonl");
        let full_reader = BufReader::new(File::open(&full_file)?);
        let full_lines: Vec<String> = full_reader.lines().collect::<Result<_, _>>()?;
        assert_eq!(full_lines.len(), 2);

        let step2_full: TrajectoryStep = serde_json::from_str(&full_lines[1])?;
        assert_eq!(step2_full.content, long_content);
        assert!(step2_full.truncated_fields.is_none());

        // Test checkpointing
        let cp_id = logger.save_checkpoint(
            "test_point",
            Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"),
        )?;
        let checkpoints = logger.get_checkpoints()?;
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].id, cp_id);
        assert_eq!(checkpoints[0].step_index, 2);
        assert_eq!(checkpoints[0].label, "test_point");

        // Clean up
        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[tokio::test]
    async fn test_truncation_rollback() -> anyhow::Result<()> {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_test_rollback_{}", uuid::Uuid::new_v4()));
        let mut logger = TrajectoryLogger::new(&temp_dir)?;

        for i in 1..=5 {
            logger
                .log_step(
                    "STEP",
                    "USER",
                    &format!("Turn {}", i),
                    serde_json::json!({}),
                )
                .await?;
        }
        assert_eq!(logger.step_count(), 5);

        // Truncate after turn 2
        logger.truncate_after(2)?;
        assert_eq!(logger.step_count(), 2);

        let s3 = logger.read_step_full(3)?;
        assert!(s3.is_none());

        let s2 = logger.read_step_full(2)?;
        assert!(s2.is_some());
        assert_eq!(s2.unwrap().content, "Turn 2");

        // Verify JSONL files updated
        let compact_reader = BufReader::new(File::open(temp_dir.join("trajectory.jsonl"))?);
        let lines: Vec<String> = compact_reader.lines().collect::<Result<_, _>>()?;
        assert_eq!(lines.len(), 2);

        let _ = std::fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
