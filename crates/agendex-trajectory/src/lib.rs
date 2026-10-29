pub mod compactor;
pub mod search;
pub mod secrets;
pub mod snapshots;
pub mod telemetry;
pub mod trajectory;

pub use secrets::{mask_secrets, redact_json_value, StreamingSecretFilter};

pub use compactor::{
    CompactedTrajectory, CompactorConfig, CompactorEngine, ContinuationSummarySections,
    ImmunityState, ModelTier, DEFAULT_HEADROOM_RATIO, DEFAULT_HEAD_TURNS, DEFAULT_IMMUNITY_TURNS,
    DEFAULT_TAIL_TURNS, STRATEGY_ANCHORED_HEAD_TAIL,
};
pub use search::{HistorySearchQuery, HistorySearchResult, TrajectorySearcher};
pub use snapshots::{state_dir_for_workspace, SnapshotEngine, SnapshotRecord, TurnRollbackResult};
pub use telemetry::{
    estimate_cost, estimate_cost_optional, format_comma, format_telemetry, is_known_pricing_model,
    SessionUsage, TokenUsage,
};
pub use trajectory::{
    CheckpointRecord, TrajectoryLogger, TrajectoryStep, TrajectoryStepInput,
    DEFAULT_COMPACT_CHAR_LIMIT,
};
