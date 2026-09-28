//! The Valka engine: in-RAM shard state machines fed by the WAL.
//!
//! Every mutation goes through [`Engine`]: validate against RAM state under the shard
//! lock, build a record, apply it, append it to the WAL, release the lock, await
//! durability, then acknowledge. Reads are served from RAM. Recovery rebuilds RAM from
//! snapshots plus WAL replay. See `docs/wal/DESIGN.md`.

pub mod apply;
mod checkpoints;
pub mod clock;
pub mod engine;
pub mod ingest;
mod loops;
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
pub use engine::{CreateTask, DispatchInfo, Engine, EngineConfig, EngineEvent, FailResult};
pub use ingest::LogIngester;
pub use sink::{DispatchableTask, NoopSink, OfferOutcome, TaskSink};
pub use stats::{NodeStats, ShardDetail, ShardStats, TaskCounts};
pub use view::{CheckpointView, DeadLetterView, RunView, SignalView, TaskView};

#[cfg(test)]
mod tests;
