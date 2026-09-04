//! Write-ahead log on an object store.
//!
//! This crate owns every byte format that lands in the bucket: WAL segments, shard
//! snapshots, the assignment object and task log chunks. It knows nothing about task
//! semantics beyond the record enum; applying records to state is `valka-engine`'s job.
//!
//! See `docs/wal/DESIGN.md`.

pub mod error;
pub mod fault;
pub mod logstore;
pub mod lsn;
pub mod ownership;
pub mod reader;
pub mod record;
pub mod segment;
pub mod snapshot;
pub mod store;
pub mod writer;

pub use error::WalError;
pub use fault::{FaultConfig, FaultyStore};
pub use lsn::Lsn;
pub use ownership::{Ownership, OwnershipCheck, OwnershipVerdict, SingleNodeOwnership};
pub use record::{Envelope, FailureOutcome, TaskSpec, WalRecord};
pub use store::Store;
pub use writer::{Durable, WalWriter, WalWriterConfig};
