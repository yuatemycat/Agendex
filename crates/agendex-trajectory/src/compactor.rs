use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::trajectory::TrajectoryStep;

/// Default headroom buffer fraction (15%) reserved before reaching model ceiling.
pub const DEFAULT_HEADROOM_RATIO: f64 = 0.15;

/// Default turns preserved at the head of the trajectory (initial user prompt anchors).
pub const DEFAULT_HEAD_TURNS: usize = 2;

/// Default turns preserved at the tail of the trajectory (recent execution context).
pub const DEFAULT_TAIL_TURNS: usize = 4;

/// Default number of post-compaction immunity turns granted.
pub const DEFAULT_IMMUNITY_TURNS: usize = 1;

/// Strategy identifier for anchored head-tail context compaction.
pub const STRATEGY_ANCHORED_HEAD_TAIL: &str = "ANCHORED_HEAD_TAIL";

/// Model context token capacity tiers supporting standard industry context windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelTier {
    Tier32k,
    Tier64k,
    Compact80k,
    Standard128k,
    Tier200k,
    HighTier256k,
    Extended840k,
    Tier1M,
    Tier2M,
    Custom(usize),
}

impl ModelTier {
    /// Return the raw token ceiling for this tier.
    pub fn ceiling_tokens(&self) -> usize {
        match self {
            ModelTier::Tier32k => 32_000,
            ModelTier::Tier64k => 64_000,
            ModelTier::Compact80k => 80_000,
            ModelTier::Standard128k => 128_000,
            ModelTier::Tier200k => 200_000,
            ModelTier::HighTier256k => 256_000,
            ModelTier::Extended840k => 840_000,
            ModelTier::Tier1M => 1_000_000,
            ModelTier::Tier2M => 2_000_000,
            ModelTier::Custom(c) => *c,
        }
    }
}

/// Post-compaction immunity state machine preventing infinite compaction churn loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImmunityState {
    /// Immunity is inactive; token ceilings are actively evaluated.
    Inactive,
    /// Immunity is active for `turns_remaining` turns. While active, `should_compact` returns false.
    Active { turns_remaining: usize },
}

/// Structured content for the 3-section context continuation summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuationSummarySections {
    pub task_overview: String,
    pub key_decisions: Vec<String>,
    pub next_steps: Vec<String>,
}

/// Configuration settings for the context compactor engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactorConfig {
    pub model_tier: ModelTier,
    pub headroom_ratio: f64,
    pub head_turns: usize,
    pub tail_turns: usize,
    pub immunity_turns: usize,
}

impl Default for CompactorConfig {
    fn default() -> Self {
        Self {
            model_tier: ModelTier::Standard128k,
            headroom_ratio: DEFAULT_HEADROOM_RATIO,
            head_turns: DEFAULT_HEAD_TURNS,
            tail_turns: DEFAULT_TAIL_TURNS,
            immunity_turns: DEFAULT_IMMUNITY_TURNS,
        }
    }
}

/// Result of executing head-tail trajectory compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactedTrajectory {
    pub strategy: String,
    pub retained_steps: Vec<TrajectoryStep>,
    pub evicted_count: usize,
    pub continuation_summary: String,
    pub tokens_before: usize,
}

/// Head-tail context compaction engine implementing user-anchored head-tail pruning.
pub struct CompactorEngine {
    config: CompactorConfig,
    immunity: ImmunityState,
    compaction_count: usize,
}

impl CompactorEngine {
    /// Initialize a new compactor engine for the specified model tier with default settings.
    pub fn new(tier: ModelTier) -> Self {
        Self {
            config: CompactorConfig {
                model_tier: tier,
                ..Default::default()
            },
            immunity: ImmunityState::Inactive,
            compaction_count: 0,
        }
    }

    /// Initialize with full custom configuration.
    pub fn with_config(config: CompactorConfig) -> Self {
        Self {
            config,
            immunity: ImmunityState::Inactive,
            compaction_count: 0,
        }
    }

    /// Return the raw token ceiling for the configured tier.
    pub fn ceiling_tokens(&self) -> usize {
        self.config.model_tier.ceiling_tokens()
    }

    /// Return the effective threshold at which compaction triggers (ceiling minus headroom).
    pub fn effective_threshold(&self) -> usize {
        let ceiling = self.ceiling_tokens() as f64;
        let threshold = ceiling * (1.0 - self.config.headroom_ratio);
        threshold.round() as usize
    }

    /// Check if active context tokens have reached or breached the compaction threshold.
    /// Returns `false` if post-compaction immunity is currently active.
    pub fn should_compact(&self, current_tokens: usize) -> bool {
        if self.has_immunity() {
            tracing::debug!(
                "Compaction suppressed by post-compaction immunity state: {:?}",
                self.immunity
            );
            return false;
        }

        current_tokens >= self.effective_threshold()
    }

    /// Query whether post-compaction immunity is currently active.
    pub fn has_immunity(&self) -> bool {
        matches!(self.immunity, ImmunityState::Active { turns_remaining } if turns_remaining > 0)
    }

    /// Explicitly grant post-compaction immunity for N turns.
    pub fn grant_immunity(&mut self, turns: usize) {
        self.immunity = if turns > 0 {
            ImmunityState::Active {
                turns_remaining: turns,
            }
        } else {
            ImmunityState::Inactive
        };
    }

    /// Clear post-compaction immunity immediately.
    pub fn clear_immunity(&mut self) {
        self.immunity = ImmunityState::Inactive;
    }

    /// Advance the immunity state machine by one turn (e.g., at the conclusion of an agent turn).
    pub fn advance_turn(&mut self) {
        if let ImmunityState::Active { turns_remaining } = self.immunity {
            if turns_remaining <= 1 {
                self.immunity = ImmunityState::Inactive;
                tracing::debug!("Post-compaction immunity expired. Threshold monitoring active.");
            } else {
                self.immunity = ImmunityState::Active {
                    turns_remaining: turns_remaining - 1,
                };
            }
        }
    }

    /// Generate the canonical 3-section structured continuation summary from contextual notes.
    pub fn generate_continuation_summary(&mut self, context_notes: &str) -> String {
        let sections = ContinuationSummarySections {
            task_overview: format!(
                "Ongoing agent execution session. Active task context: {}",
                context_notes.trim()
            ),
            key_decisions: vec![
                "Preserved initial user prompt and system constraints in head anchors.".to_string(),
                "Maintained recent tool execution outputs in tail context buffer.".to_string(),
                "Evicted intermediate tool observations to restore token capacity.".to_string(),
            ],
            next_steps: vec![
                "Continue executing next planned actions without context truncation.".to_string(),
                "Verify task completion against initial user objectives.".to_string(),
            ],
        };

        self.generate_structured_summary(&sections)
    }

    /// Generate a 3-section structured continuation summary from concrete section inputs.
    pub fn generate_structured_summary(
        &mut self,
        sections: &ContinuationSummarySections,
    ) -> String {
        // Automatically grant immunity upon generating continuation summary
        self.grant_immunity(self.config.immunity_turns);

        let mut output = String::new();
        output.push_str("## Context Continuation Summary (Compacted)\n\n");

        output.push_str("### 1. Task Overview & Initial Objectives\n");
        output.push_str(&sections.task_overview);
        output.push_str("\n\n");

        output.push_str("### 2. Key Decisions & Structural Changes Made\n");
        if sections.key_decisions.is_empty() {
            output.push_str("- Baseline initialization established.\n");
        } else {
            for dec in &sections.key_decisions {
                output.push_str(&format!("- {}\n", dec));
            }
        }
        output.push('\n');

        output.push_str("### 3. Current State & Next Steps\n");
        if sections.next_steps.is_empty() {
            output.push_str("- Proceed with next planned prompt turn.\n");
        } else {
            for step in &sections.next_steps {
                output.push_str(&format!("- {}\n", step));
            }
        }

        output
    }

    /// Synthesize continuation summary sections automatically by analyzing evicted trajectory steps.
    pub fn synthesize_sections_from_steps(
        &self,
        evicted_steps: &[TrajectoryStep],
        overall_steps: &[TrajectoryStep],
    ) -> ContinuationSummarySections {
        // 1. Task Overview: extract from earliest USER_INPUT
        let task_overview = overall_steps
            .iter()
            .find(|s| s.step_type == "USER_INPUT" || s.source == "USER")
            .map(|s| s.content.clone())
            .unwrap_or_else(|| "User-directed engineering and task automation.".to_string());

        // 2. Key Decisions: extract from tool actions, edits, and file operations in evicted turns
        let mut key_decisions = Vec::new();
        for step in evicted_steps {
            if step.step_type == "TOOL_CALL" || step.step_type == "TOOL_RESULT" {
                if let Some(ref meta) = step.metadata {
                    if let Some(tool) = meta.get("tool").and_then(|t| t.as_str()) {
                        key_decisions.push(format!("Executed tool `{}`: {}", tool, step.status));
                    }
                }
            } else if step.step_type == "COMPACTION" {
                key_decisions.push("Prior context compaction checkpoint integrated.".to_string());
            }
        }

        if key_decisions.is_empty() {
            key_decisions
                .push("Executed intermediate validation and code inspection turns.".to_string());
        }

        // 3. Next Steps: determine based on final evicted turns
        let mut next_steps = Vec::new();
        if let Some(last) = evicted_steps.last() {
            next_steps.push(format!(
                "Resume execution following last completed step [{}] ({})",
                last.step_index, last.step_type
            ));
        } else {
            next_steps.push("Continue with active planned objectives.".to_string());
        }

        ContinuationSummarySections {
            task_overview,
            key_decisions,
            next_steps,
        }
    }

    /// Perform head-tail trajectory compaction using user-anchored head-tail retention.
    pub fn compact_trajectory(
        &mut self,
        steps: &[TrajectoryStep],
        simulated_tokens_before: usize,
        summary_override: Option<&str>,
    ) -> CompactedTrajectory {
        let total = steps.len();
        let head_len = self.config.head_turns.min(total);
        let tail_len = self.config.tail_turns.min(total.saturating_sub(head_len));

        // If the trajectory is shorter than the combined head and tail bounds, retain as is
        if total <= self.config.head_turns + self.config.tail_turns {
            return CompactedTrajectory {
                strategy: STRATEGY_ANCHORED_HEAD_TAIL.to_string(),
                retained_steps: steps.to_vec(),
                evicted_count: 0,
                continuation_summary: String::new(),
                tokens_before: simulated_tokens_before,
            };
        }

        let head = &steps[..head_len];
        let tail = &steps[total - tail_len..];
        let middle = &steps[head_len..total - tail_len];
        let evicted_count = middle.len();

        let summary = match summary_override {
            Some(s) => {
                self.grant_immunity(self.config.immunity_turns);
                s.to_string()
            }
            None => {
                let sections = self.synthesize_sections_from_steps(middle, steps);
                self.generate_structured_summary(&sections)
            }
        };

        // Construct synthetic compaction anchor step
        let anchor_index = middle
            .last()
            .map(|s| s.step_index)
            .unwrap_or(head_len as u64);
        let compaction_step = TrajectoryStep {
            step_index: anchor_index,
            source: "SYSTEM".to_string(),
            step_type: "COMPACTION".to_string(),
            status: "DONE".to_string(),
            created_at: Utc::now().to_rfc3339(),
            content: summary.clone(),
            thinking: None,
            tool_calls: None,
            media: None,
            metadata: Some(serde_json::json!({
                "strategy": STRATEGY_ANCHORED_HEAD_TAIL,
                "evicted_turns": evicted_count,
                "head_turns": head.len(),
                "tail_turns": tail.len(),
            })),
            truncated_fields: None,
        };

        let mut retained = Vec::with_capacity(head.len() + 1 + tail.len());
        retained.extend_from_slice(head);
        retained.push(compaction_step);
        retained.extend_from_slice(tail);

        self.compaction_count += 1;

        CompactedTrajectory {
            strategy: STRATEGY_ANCHORED_HEAD_TAIL.to_string(),
            retained_steps: retained,
            evicted_count,
            continuation_summary: summary,
            tokens_before: simulated_tokens_before,
        }
    }

    /// Return total compaction cycles triggered.
    pub fn compaction_count(&self) -> usize {
        self.compaction_count
    }

    /// Return current immunity state.
    pub fn immunity_state(&self) -> ImmunityState {
        self.immunity
    }

    /// Access active configuration.
    pub fn config(&self) -> &CompactorConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_token_ceilings_and_effective_thresholds() {
        // 80k: ceiling 80,000, threshold at 85% = 68,000
        let c80 = CompactorEngine::new(ModelTier::Compact80k);
        assert_eq!(c80.ceiling_tokens(), 80_000);
        assert_eq!(c80.effective_threshold(), 68_000);
        assert!(!c80.should_compact(67_999));
        assert!(c80.should_compact(68_000));
        assert!(c80.should_compact(80_000));

        // 128k: ceiling 128,000, threshold at 85% = 108,800
        let c128 = CompactorEngine::new(ModelTier::Standard128k);
        assert_eq!(c128.ceiling_tokens(), 128_000);
        assert_eq!(c128.effective_threshold(), 108_800);
        assert!(!c128.should_compact(108_799));
        assert!(c128.should_compact(108_800));

        // 256k: ceiling 256,000, threshold at 85% = 217,600
        let c256 = CompactorEngine::new(ModelTier::HighTier256k);
        assert_eq!(c256.ceiling_tokens(), 256_000);
        assert_eq!(c256.effective_threshold(), 217_600);

        // 840k: ceiling 840,000, threshold at 85% = 714,000
        let c840 = CompactorEngine::new(ModelTier::Extended840k);
        assert_eq!(c840.ceiling_tokens(), 840_000);
        assert_eq!(c840.effective_threshold(), 714_000);
    }

    #[test]
    fn test_post_compaction_immunity_state_machine() {
        let mut comp = CompactorEngine::new(ModelTier::Compact80k);
        assert_eq!(comp.immunity_state(), ImmunityState::Inactive);
        assert!(!comp.has_immunity());

        // Breaching threshold triggers compaction check
        assert!(comp.should_compact(70_000));

        // Generating continuation summary activates immunity
        let summary = comp.generate_continuation_summary("Unit test turn in progress.");
        assert!(summary.contains("### 1. Task Overview & Initial Objectives"));
        assert!(summary.contains("### 2. Key Decisions & Structural Changes Made"));
        assert!(summary.contains("### 3. Current State & Next Steps"));

        // Immunity is now active
        assert_eq!(
            comp.immunity_state(),
            ImmunityState::Active { turns_remaining: 1 }
        );
        assert!(comp.has_immunity());

        // While immune, should_compact returns false even if token count exceeds threshold
        assert!(!comp.should_compact(75_000));
        assert!(!comp.should_compact(90_000));

        // Advance turn: immunity expires
        comp.advance_turn();
        assert_eq!(comp.immunity_state(), ImmunityState::Inactive);
        assert!(!comp.has_immunity());

        // Now should_compact evaluates tokens again
        assert!(comp.should_compact(70_000));
    }

    #[test]
    fn test_head_tail_compaction_execution() {
        let mut comp = CompactorEngine::new(ModelTier::Standard128k);

        // Build 10 simulated steps
        let mut steps = Vec::new();
        for i in 1..=10 {
            steps.push(TrajectoryStep {
                step_index: i,
                source: if i == 1 {
                    "USER".to_string()
                } else {
                    "PLANNER".to_string()
                },
                step_type: if i == 1 {
                    "USER_INPUT".to_string()
                } else {
                    "TOOL_CALL".to_string()
                },
                status: "DONE".to_string(),
                created_at: Utc::now().to_rfc3339(),
                content: format!("Step content {}", i),
                thinking: None,
                tool_calls: None,
                media: None,
                metadata: Some(serde_json::json!({"turn": i})),
                truncated_fields: None,
            });
        }

        // Compact with default head=2, tail=4 (evicting 4 middle turns: 3, 4, 5, 6)
        let compacted = comp.compact_trajectory(&steps, 115_000, None);
        assert_eq!(compacted.strategy, STRATEGY_ANCHORED_HEAD_TAIL);
        assert_eq!(compacted.evicted_count, 4);

        // Retained: 2 head + 1 compaction anchor + 4 tail = 7 steps
        assert_eq!(compacted.retained_steps.len(), 7);

        // Head preserved
        assert_eq!(compacted.retained_steps[0].step_index, 1);
        assert_eq!(compacted.retained_steps[1].step_index, 2);

        // Anchor in middle
        assert_eq!(compacted.retained_steps[2].step_type, "COMPACTION");
        assert!(compacted.retained_steps[2]
            .content
            .contains("Context Continuation Summary"));

        // Tail preserved
        assert_eq!(compacted.retained_steps[3].step_index, 7);
        assert_eq!(compacted.retained_steps[4].step_index, 8);
        assert_eq!(compacted.retained_steps[5].step_index, 9);
        assert_eq!(compacted.retained_steps[6].step_index, 10);

        // Immunity was activated by compaction
        assert!(comp.has_immunity());
    }
}
