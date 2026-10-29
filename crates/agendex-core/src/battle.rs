use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::swarm::{SubagentInstance, SubagentManager, SubagentPersona, SubagentStatus};
use crate::worktree::{WorkspaceMode, WorktreeManager};

/// Configuration for an adversarial or competitive duel between two subagents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BattleDuelConfig {
    pub task_prompt: String,
    pub combatant_a: SubagentPersona,
    pub combatant_b: SubagentPersona,
    pub verification_cmd: Option<String>,
    pub workspace_mode: WorkspaceMode,
    pub apply_winner: bool,
    pub cleanup_worktrees: bool,
    pub timeout: Option<Duration>,
}

impl Default for BattleDuelConfig {
    fn default() -> Self {
        Self {
            task_prompt: String::new(),
            combatant_a: SubagentPersona::coder(),
            combatant_b: SubagentPersona::coder(),
            verification_cmd: None,
            workspace_mode: WorkspaceMode::Share,
            apply_winner: true,
            cleanup_worktrees: true,
            timeout: Some(Duration::from_secs(60)),
        }
    }
}

/// Final outcome verdict of a Colosseum duel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BattleVerdict {
    Winner {
        persona_name: String,
        agent_id: String,
    },
    Tie,
    NoWinner,
}

/// Evaluation record for an individual combatant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BattleCombatantResult {
    pub persona_name: String,
    pub agent_id: String,
    pub output_text: String,
    pub workspace_path: PathBuf,
    pub diff: String,
    pub verification_passed: bool,
    pub verification_output: String,
    pub execution_duration: Duration,
}

/// Comprehensive outcome report of a Colosseum duel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BattleReport {
    pub verdict: BattleVerdict,
    pub rationale: String,
    pub combatant_a: BattleCombatantResult,
    pub combatant_b: BattleCombatantResult,
    pub applied_to_parent: bool,
}

/// Colosseum Battle Arena orchestrating adversarial duels between two subagents.
pub struct BattleArena {
    subagent_mgr: Arc<SubagentManager>,
    worktree_mgr: Arc<WorktreeManager>,
}

impl BattleArena {
    pub fn new(subagent_mgr: Arc<SubagentManager>, worktree_mgr: Arc<WorktreeManager>) -> Self {
        Self {
            subagent_mgr,
            worktree_mgr,
        }
    }

    /// Execute a duel between combatant A and combatant B on the given task.
    pub async fn duel(&self, config: BattleDuelConfig) -> anyhow::Result<BattleReport> {
        let name_a = config.combatant_a.name.clone();
        let name_b = config.combatant_b.name.clone();

        // 1. Spawn and run combatant A in isolated workspace
        let start_a = Instant::now();
        let id_a = self
            .subagent_mgr
            .spawn_custom_subagent(
                config.combatant_a.clone(),
                &config.task_prompt,
                config.workspace_mode,
            )
            .await?;

        let (inst_a, dur_a) = match self
            .subagent_mgr
            .wait_for_subagent(&id_a, config.timeout)
            .await
        {
            Ok(inst) => (inst, start_a.elapsed()),
            Err(e) => {
                let dummy = SubagentInstance {
                    id: id_a.clone(),
                    persona: config.combatant_a.clone(),
                    status: SubagentStatus::Failed,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    workspace_dir: PathBuf::new(),
                    workspace_mode: config.workspace_mode,
                    result: None,
                    error: Some(format!("Combatant A failed to finish: {}", e)),
                };
                (dummy, start_a.elapsed())
            }
        };

        // 2. Spawn and run combatant B in isolated workspace
        let start_b = Instant::now();
        let id_b = self
            .subagent_mgr
            .spawn_custom_subagent(
                config.combatant_b.clone(),
                &config.task_prompt,
                config.workspace_mode,
            )
            .await?;

        let (inst_b, dur_b) = match self
            .subagent_mgr
            .wait_for_subagent(&id_b, config.timeout)
            .await
        {
            Ok(inst) => (inst, start_b.elapsed()),
            Err(e) => {
                let dummy = SubagentInstance {
                    id: id_b.clone(),
                    persona: config.combatant_b.clone(),
                    status: SubagentStatus::Failed,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    workspace_dir: PathBuf::new(),
                    workspace_mode: config.workspace_mode,
                    result: None,
                    error: Some(format!("Combatant B failed to finish: {}", e)),
                };
                (dummy, start_b.elapsed())
            }
        };

        // 3. Retrieve workspaces and compute diffs
        let ws_a_opt = self.worktree_mgr.get_workspace(&id_a).await;
        let ws_b_opt = self.worktree_mgr.get_workspace(&id_b).await;

        let diff_a = if let Some(ws) = &ws_a_opt {
            self.worktree_mgr.get_diff(ws).unwrap_or_default()
        } else {
            String::new()
        };

        let diff_b = if let Some(ws) = &ws_b_opt {
            self.worktree_mgr.get_diff(ws).unwrap_or_default()
        } else {
            String::new()
        };

        // 4. Verification step
        let (passed_a, verify_out_a) =
            if let (Some(cmd), Some(ws)) = (&config.verification_cmd, &ws_a_opt) {
                run_verification(&ws.path, cmd).await
            } else {
                (inst_a.status == SubagentStatus::Completed, String::new())
            };

        let (passed_b, verify_out_b) =
            if let (Some(cmd), Some(ws)) = (&config.verification_cmd, &ws_b_opt) {
                run_verification(&ws.path, cmd).await
            } else {
                (inst_b.status == SubagentStatus::Completed, String::new())
            };

        // 5. Select winner with structured rationale
        let (verdict, rationale) = if passed_a && !passed_b {
            (
                BattleVerdict::Winner {
                    persona_name: name_a.clone(),
                    agent_id: id_a.clone(),
                },
                format!(
                    "Combatant A ({}) passed verification while Combatant B ({}) failed.",
                    name_a, name_b
                ),
            )
        } else if !passed_a && passed_b {
            (
                BattleVerdict::Winner {
                    persona_name: name_b.clone(),
                    agent_id: id_b.clone(),
                },
                format!(
                    "Combatant B ({}) passed verification while Combatant A ({}) failed.",
                    name_b, name_a
                ),
            )
        } else if !passed_a && !passed_b {
            (
                BattleVerdict::NoWinner,
                format!(
                    "Both combatants ({}, {}) failed verification. No patch applied.",
                    name_a, name_b
                ),
            )
        } else {
            // Both passed! Compare diffs and durations
            if diff_a == diff_b {
                (
                    BattleVerdict::Tie,
                    format!(
                        "Both combatants ({}, {}) produced identical passing patches.",
                        name_a, name_b
                    ),
                )
            } else if dur_a < dur_b {
                (
                    BattleVerdict::Winner {
                        persona_name: name_a.clone(),
                        agent_id: id_a.clone(),
                    },
                    format!(
                        "Both passed verification. Combatant A ({}) was faster ({:?} vs {:?}).",
                        name_a, dur_a, dur_b
                    ),
                )
            } else if dur_b < dur_a {
                (
                    BattleVerdict::Winner {
                        persona_name: name_b.clone(),
                        agent_id: id_b.clone(),
                    },
                    format!(
                        "Both passed verification. Combatant B ({}) was faster ({:?} vs {:?}).",
                        name_b, dur_b, dur_a
                    ),
                )
            } else {
                (
                    BattleVerdict::Tie,
                    format!(
                        "Both combatants ({}, {}) passed verification with equal metrics.",
                        name_a, name_b
                    ),
                )
            }
        };

        // 6. Apply winner changes if configured
        let mut applied_to_parent = false;
        if config.apply_winner {
            match &verdict {
                BattleVerdict::Winner { agent_id, .. } => {
                    let winning_ws_opt = if agent_id == &id_a {
                        ws_a_opt.as_ref()
                    } else {
                        ws_b_opt.as_ref()
                    };

                    if let Some(ws) = winning_ws_opt {
                        self.worktree_mgr.apply_to_base(ws).await?;
                        applied_to_parent = true;
                    }
                }
                _ => {
                    // Tie or NoWinner -> apply nothing
                    applied_to_parent = false;
                }
            }
        }

        let result_a = BattleCombatantResult {
            persona_name: name_a,
            agent_id: id_a.clone(),
            output_text: inst_a
                .result
                .unwrap_or_else(|| inst_a.error.unwrap_or_default()),
            workspace_path: inst_a.workspace_dir,
            diff: diff_a,
            verification_passed: passed_a,
            verification_output: verify_out_a,
            execution_duration: dur_a,
        };

        let result_b = BattleCombatantResult {
            persona_name: name_b,
            agent_id: id_b.clone(),
            output_text: inst_b
                .result
                .unwrap_or_else(|| inst_b.error.unwrap_or_default()),
            workspace_path: inst_b.workspace_dir,
            diff: diff_b,
            verification_passed: passed_b,
            verification_output: verify_out_b,
            execution_duration: dur_b,
        };

        // 7. Cleanup worktrees if requested
        if config.cleanup_worktrees {
            let _ = self.worktree_mgr.cleanup_workspace(&id_a).await;
            let _ = self.worktree_mgr.cleanup_workspace(&id_b).await;
        }

        Ok(BattleReport {
            verdict,
            rationale,
            combatant_a: result_a,
            combatant_b: result_b,
            applied_to_parent,
        })
    }
}

/// Execute a verification shell command in a workspace directory.
async fn run_verification(workspace_path: &Path, cmd_str: &str) -> (bool, String) {
    if !workspace_path.exists() {
        return (
            false,
            format!(
                "Workspace path does not exist: {}",
                workspace_path.display()
            ),
        );
    }

    if let crate::safety::CommandSafetyVerdict::Blocked { reason } =
        crate::safety::CommandSafetyAssessor::evaluate(cmd_str)
    {
        return (
            false,
            format!("Verification command blocked by safety policy: {}", reason),
        );
    }

    #[cfg(target_os = "windows")]
    let mut cmd = tokio::process::Command::new("cmd");
    #[cfg(target_os = "windows")]
    cmd.args(["/C", cmd_str]);

    #[cfg(not(target_os = "windows"))]
    let mut cmd = tokio::process::Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    cmd.args(["-c", cmd_str]);

    cmd.current_dir(workspace_path);
    match cmd.output().await {
        Ok(out) => {
            let success = out.status.success();
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            if !out.stderr.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str("STDERR: ");
                text.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            (success, text)
        }
        Err(e) => (
            false,
            format!("Failed to launch verification command: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentConfig;
    use crate::mailbox::MailboxManager;

    #[tokio::test]
    async fn test_battle_arena_verdict_logic() -> anyhow::Result<()> {
        let test_root =
            std::env::temp_dir().join(format!("agendex_battle_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&test_root)?;

        let mailbox = Arc::new(MailboxManager::new());
        let worktree_mgr = Arc::new(WorktreeManager::new(test_root.clone(), None)?);
        let base_config = AgentConfig {
            workspace_dir: test_root.clone(),
            ..Default::default()
        };

        let subagent_mgr = Arc::new(SubagentManager::new(
            "parent",
            base_config,
            Arc::clone(&mailbox),
            Arc::clone(&worktree_mgr),
        ));

        let _arena = BattleArena::new(subagent_mgr, worktree_mgr);

        // Verification command that succeeds
        #[cfg(target_os = "windows")]
        let verify_success = "cmd /C exit 0";
        #[cfg(not(target_os = "windows"))]
        let verify_success = "exit 0";

        let (passed, _) = run_verification(&test_root, verify_success).await;
        assert!(passed);

        // Verification command that fails
        #[cfg(target_os = "windows")]
        let verify_fail = "cmd /C exit 1";
        #[cfg(not(target_os = "windows"))]
        let verify_fail = "exit 1";

        let (failed, _) = run_verification(&test_root, verify_fail).await;
        assert!(!failed);

        let _ = std::fs::remove_dir_all(&test_root);
        Ok(())
    }
}
