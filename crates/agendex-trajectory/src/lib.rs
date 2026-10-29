pub mod compactor;
pub mod snapshots;
pub mod trajectory;

pub use compactor::{
    CompactedTrajectory, CompactorConfig, CompactorEngine, ContinuationSummarySections,
    ImmunityState, ModelTier, DEFAULT_HEADROOM_RATIO, DEFAULT_HEAD_TURNS, DEFAULT_IMMUNITY_TURNS,
    DEFAULT_TAIL_TURNS, STRATEGY_ANCHORED_HEAD_TAIL,
};
pub use snapshots::{state_dir_for_workspace, SnapshotEngine, SnapshotRecord, TurnRollbackResult};
pub use trajectory::{
    CheckpointRecord, TrajectoryLogger, TrajectoryStep, TrajectoryStepInput,
    DEFAULT_COMPACT_CHAR_LIMIT,
};
