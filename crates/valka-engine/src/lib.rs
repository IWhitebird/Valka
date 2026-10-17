//! The Valka engine: in-RAM shard state machines fed by the WAL.
//!
//! Every mutation goes through [`Engine`]: validate against RAM state under the shard
//! lock, build a record, apply it, append it to the WAL, release the lock, await
//! durability, then acknowledge. Reads are served from RAM. Shards move between nodes by
//! claim and release; a dead node's log is split into per-shard snapshots. See
//! `docs/wal/DESIGN.md` and `docs/wal/PHASE2.md`.

pub mod apply;
mod checkpoints;
pub mod clock;
pub mod cluster;
pub mod engine;
pub mod ingest;
mod loops;
mod ownership;
mod pending;
mod recovery;
pub mod retry;
mod signals;
pub mod sink;
pub mod state;
pub mod stats;
mod tasks;
pub mod timers;
pub mod view;
mod write_path;

pub use checkpoints::{MAX_CHECKPOINT_BYTES, MAX_CHECKPOINTS_PER_TASK, MAX_STEP_NAME_LEN};
pub use clock::{Clock, TokioClock};
pub use cluster::{GcReport, MemberInfo};
pub use engine::{
    CreateTask, DispatchInfo, Engine, EngineConfig, EngineEvent, FailResult, ResultOutcome,
    RunLocation, RunResult,
};
pub use ingest::LogIngester;
pub use ownership::ClaimRefused;
pub use sink::{DispatchableTask, NoopSink, OfferOutcome, TaskSink};
pub use state::ShardPhase;
pub use stats::{NodeStats, ShardDetail, ShardStats, TaskCounts};
pub use view::{CheckpointView, DeadLetterView, RunView, SignalView, TaskView};

#[cfg(test)]
mod cluster_tests;
#[cfg(test)]
mod safety_tests;
#[cfg(test)]
mod tests;
