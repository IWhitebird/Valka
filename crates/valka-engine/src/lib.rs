//! The Valka engine: in-RAM shard state machines fed by the WAL.
//!
//! Every mutation goes through [`Engine`]: validate against RAM state under the shard
//! lock, build a record, apply it, append it to the WAL, release the lock, await
//! durability, then acknowledge. Reads are served from RAM. Recovery rebuilds RAM from
//! snapshots plus WAL replay. See `docs/wal/DESIGN.md`.

pub mod clock;
pub mod engine;
pub mod ingest;
mod loops;
mod recovery;
pub mod retry;
pub mod sink;
pub mod state;
pub mod timers;
pub mod view;

pub use clock::{Clock, TokioClock};
pub use engine::{CreateTask, DispatchInfo, Engine, EngineConfig, EngineEvent, FailResult};
pub use ingest::LogIngester;
pub use sink::{DispatchableTask, NoopSink, OfferOutcome, TaskSink};
pub use view::{DeadLetterView, RunView, SignalView, TaskView};

#[cfg(test)]
mod tests;
