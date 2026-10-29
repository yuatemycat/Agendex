use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::trajectory::TrajectoryStep;

/// Filter and search parameters for querying trajectory history.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HistorySearchQuery {
    /// Keyword or phrase to search for across step content, tool calls, and thinking.
    pub query: String,
    /// Optional session ID filter. If omitted, searches all recorded sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional step type filter (e.g., "USER_INPUT", "TOOL_RESULT", "ASSISTANT").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_type: Option<String>,
    /// Maximum number of matching steps to return (default: 20).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// A matched trajectory step returned from history search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistorySearchResult {
    pub session_id: String,
    pub step_index: u64,
    pub step_type: String,
    pub source: String,
    pub created_at: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    pub matched_in: Vec<String>,
}

/// Cross-session trajectory search engine across SQLite databases and JSONL archives.
pub struct TrajectorySearcher;

impl TrajectorySearcher {
    /// Search across past trajectory sessions in `logs_dir`.
    pub fn search(
        logs_dir: &Path,
        query: &HistorySearchQuery,
    ) -> anyhow::Result<Vec<HistorySearchResult>> {
        let max_results = query.limit.unwrap_or(20).max(1);
        let mut results = Vec::new();

        if !logs_dir.exists() || !logs_dir.is_dir() {
            return Ok(results);
        }

        // Determine target session directories
        let mut session_dirs = Vec::new();
        if let Some(ref target_session) = query.session_id {
            let direct = logs_dir.join(target_session);
            if direct.is_dir() {
                session_dirs.push((target_session.clone(), direct));
            }
        } else if let Ok(entries) = std::fs::read_dir(logs_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        session_dirs.push((name, entry.path()));
                    }
                }
            }
        }

        // Sort sessions by modification time descending (most recent first)
        session_dirs.sort_by(|a, b| {
            let time_a = std::fs::metadata(&a.1).and_then(|m| m.modified()).ok();
            let time_b = std::fs::metadata(&b.1).and_then(|m| m.modified()).ok();
            time_b.cmp(&time_a)
        });

        let query_lower = query.query.to_lowercase();

        for (session_id, dir) in session_dirs {
            if results.len() >= max_results {
                break;
            }

            let db_path = dir.join("trajectory.db");
            if db_path.exists() {
                if let Ok(mut db_results) = Self::search_sqlite_db(
                    &db_path,
                    &session_id,
                    query,
                    max_results - results.len(),
                ) {
                    results.append(&mut db_results);
                    continue;
                }
            }

            // Fallback to JSONL search if SQLite DB is absent or unreadable
            let full_jsonl = dir.join("trajectory_full.jsonl");
            let compact_jsonl = dir.join("trajectory.jsonl");
            let target_file = if full_jsonl.exists() {
                Some(full_jsonl)
            } else if compact_jsonl.exists() {
                Some(compact_jsonl)
            } else {
                None
            };

            if let Some(jsonl_path) = target_file {
                if let Ok(mut file_results) = Self::search_jsonl(
                    &jsonl_path,
                    &session_id,
                    &query_lower,
                    query.step_type.as_deref(),
                    max_results - results.len(),
                ) {
                    results.append(&mut file_results);
                }
            }
        }

        Ok(results)
    }

    /// Query a single session's SQLite database directly.
    pub fn search_sqlite_db(
        db_path: &Path,
        session_id: &str,
        query: &HistorySearchQuery,
        limit: usize,
    ) -> anyhow::Result<Vec<HistorySearchResult>> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let like_pattern = format!("%{}%", query.query);
        let mut results = Vec::new();

        let sql = match &query.step_type {
            Some(_) => {
                "SELECT step_index, step_type, source, status, content, thinking, tool_calls, created_at
                 FROM trajectory_steps
                 WHERE step_type = ?1 AND (content LIKE ?2 OR tool_calls LIKE ?2 OR thinking LIKE ?2)
                 ORDER BY step_index DESC LIMIT ?3"
            }
            None => {
                "SELECT step_index, step_type, source, status, content, thinking, tool_calls, created_at
                 FROM trajectory_steps
                 WHERE content LIKE ?1 OR tool_calls LIKE ?1 OR thinking LIKE ?1
                 ORDER BY step_index DESC LIMIT ?2"
            }
        };

        let mut stmt = conn.prepare(sql)?;
        let query_lower = query.query.to_lowercase();

        let map_row = |row: &rusqlite::Row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
            ))
        };

        let rows: Vec<_> = match &query.step_type {
            Some(st) => stmt
                .query_map(params![st, like_pattern, limit as i64], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
            None => stmt
                .query_map(params![like_pattern, limit as i64], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };

        for row in rows {
            let (
                step_index,
                step_type,
                source,
                _status,
                content,
                thinking,
                tool_calls_raw,
                created_at,
            ) = row;
            let mut matched_in = Vec::new();
            if content.to_lowercase().contains(&query_lower) {
                matched_in.push("content".to_string());
            }
            if let Some(ref th) = thinking {
                if th.to_lowercase().contains(&query_lower) {
                    matched_in.push("thinking".to_string());
                }
            }
            if let Some(ref tc) = tool_calls_raw {
                if tc.to_lowercase().contains(&query_lower) {
                    matched_in.push("tool_calls".to_string());
                }
            }

            let tool_calls = tool_calls_raw
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());
            let snippet = create_snippet(&content, &query_lower);

            results.push(HistorySearchResult {
                session_id: session_id.to_string(),
                step_index,
                step_type,
                source,
                created_at,
                snippet,
                tool_calls,
                matched_in,
            });
        }

        Ok(results)
    }

    /// Scan a JSONL trajectory file line by line.
    pub fn search_jsonl(
        file_path: &Path,
        session_id: &str,
        query_lower: &str,
        step_type_filter: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<HistorySearchResult>> {
        let file = File::open(file_path)?;
        let reader = BufReader::new(file);
        let mut results = Vec::new();

        for line_res in reader.lines() {
            if results.len() >= limit {
                break;
            }
            let line = line_res?;
            if line.trim().is_empty() {
                continue;
            }

            if let Ok(step) = serde_json::from_str::<TrajectoryStep>(&line) {
                if let Some(st) = step_type_filter {
                    if !step.step_type.eq_ignore_ascii_case(st) {
                        continue;
                    }
                }

                let mut matched_in = Vec::new();
                if step.content.to_lowercase().contains(query_lower) {
                    matched_in.push("content".to_string());
                }
                if let Some(ref th) = step.thinking {
                    if th.to_lowercase().contains(query_lower) {
                        matched_in.push("thinking".to_string());
                    }
                }
                if let Some(ref tc) = step.tool_calls {
                    if tc.to_string().to_lowercase().contains(query_lower) {
                        matched_in.push("tool_calls".to_string());
                    }
                }

                if !matched_in.is_empty() {
                    let snippet = create_snippet(&step.content, query_lower);
                    results.push(HistorySearchResult {
                        session_id: session_id.to_string(),
                        step_index: step.step_index,
                        step_type: step.step_type,
                        source: step.source,
                        created_at: step.created_at,
                        snippet,
                        tool_calls: step.tool_calls,
                        matched_in,
                    });
                }
            }
        }

        Ok(results)
    }
}

/// Helper extracting an informative contextual preview snippet around matched text.
fn create_snippet(content: &str, query_lower: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    let lower = content.to_lowercase();
    if let Some(pos) = lower.find(query_lower) {
        let start = pos.saturating_sub(60);
        let end = (pos + query_lower.len() + 100).min(content.len());
        let prefix = if start > 0 { "..." } else { "" };
        let suffix = if end < content.len() { "..." } else { "" };
        format!("{}{}{}", prefix, &content[start..end].trim(), suffix)
    } else {
        // Fallback to first 160 characters
        if content.len() > 160 {
            format!("{}...", &content[..160].trim())
        } else {
            content.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trajectory::{TrajectoryLogger, TrajectoryStepInput};

    #[tokio::test]
    async fn test_trajectory_search_across_sqlite_and_jsonl() -> anyhow::Result<()> {
        let temp_dir =
            std::env::temp_dir().join(format!("agendex_search_test_{}", uuid::Uuid::new_v4()));
        let session_1_dir = temp_dir.join("session_1");
        let session_2_dir = temp_dir.join("session_2");

        // Session 1: Create steps with TrajectoryLogger
        let mut logger_1 = TrajectoryLogger::new(&session_1_dir)?;
        logger_1
            .log_step_detailed(TrajectoryStepInput {
                step_type: "USER_INPUT".to_string(),
                source: "USER".to_string(),
                status: "DONE".to_string(),
                content: "Implement high-speed pure-TCP protocol".to_string(),
                thinking: None,
                tool_calls: None,
                media: None,
                metadata: None,
            })
            .await?;

        logger_1
            .log_step_detailed(TrajectoryStepInput {
                step_type: "TOOL_RESULT".to_string(),
                source: "SYSTEM".to_string(),
                status: "DONE".to_string(),
                content: "cargo test --package pure-tcp passed with 12 tests".to_string(),
                thinking: None,
                tool_calls: None,
                media: None,
                metadata: None,
            })
            .await?;

        // Session 2: Another goal
        let mut logger_2 = TrajectoryLogger::new(&session_2_dir)?;
        logger_2
            .log_step_detailed(TrajectoryStepInput {
                step_type: "USER_INPUT".to_string(),
                source: "USER".to_string(),
                status: "DONE".to_string(),
                content: "Optimize SQLite memory caching and compaction".to_string(),
                thinking: None,
                tool_calls: None,
                media: None,
                metadata: None,
            })
            .await?;

        // Search for "pure-TCP" across sessions
        let q = HistorySearchQuery {
            query: "pure-TCP".to_string(),
            ..Default::default()
        };
        let matches = TrajectorySearcher::search(&temp_dir, &q)?;
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].session_id, "session_1");

        // Search filtered by step_type
        let q_filtered = HistorySearchQuery {
            query: "pure-TCP".to_string(),
            step_type: Some("USER_INPUT".to_string()),
            ..Default::default()
        };
        let matches_filtered = TrajectorySearcher::search(&temp_dir, &q_filtered)?;
        assert_eq!(matches_filtered.len(), 1);
        assert_eq!(matches_filtered[0].step_type, "USER_INPUT");

        // Clean up temp directory
        let _ = std::fs::remove_dir_all(temp_dir);
        Ok(())
    }
}
