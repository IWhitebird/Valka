//! Write-ahead log on an object store.
//!
//! This crate owns every byte format that lands in the bucket: `cluster.json`, node
//! leases, shard owner records, WAL segments, shard snapshots, split markers and task log
//! chunks. It knows nothing about task semantics beyond the record enum; applying records
//! to state is `valka-engine`'s job.
//!
//! See `docs/wal/DESIGN.md` and `docs/wal/PHASE2.md`.

pub mod error;
pub mod fault;
pub mod lease;
pub mod logstore;
pub mod lsn;
pub mod meta;
pub mod owners;
pub mod placement;
pub mod queues;
pub mod reader;
pub mod record;
pub mod segment;
pub mod snapshot;
pub mod split;
pub mod store;
pub mod writer;

pub use error::WalError;
pub use fault::{FaultConfig, FaultyStore};
pub use lease::{FenceCheck, LeaseConfig, LeaseKeeper, LeaseRecord, LeaseState, NoFence};
pub use lsn::{Lsn, valid_node_id};
pub use meta::ClusterMeta;
pub use owners::{OwnerRecord, OwnerState};
pub use record::{Envelope, FailureOutcome, TaskSpec, WalRecord};
pub use snapshot::{LogPtr, SnapshotKey};
pub use store::Store;
pub use writer::{Durable, WalWriter, WalWriterConfig};
