//! Read-only statistics for operators: node health, shard map, task counts.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use valka_core::{NUM_SHARDS, ShardId, TaskStatus};

use crate::engine::Engine;
use crate::state::ShardState;

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct TaskCounts {
    pub pending: usize,
    pub running: usize,
    pub retry: usize,
    pub completed: usize,
    pub failed: usize,
    pub dead_letter: usize,
    pub cancelled: usize,
    pub total: usize,
}

impl TaskCounts {
    fn add(&mut self, status: TaskStatus) {
        match status {
            TaskStatus::Pending => self.pending += 1,
            TaskStatus::Running | TaskStatus::Dispatching => self.running += 1,
            TaskStatus::Retry => self.retry += 1,
            TaskStatus::Completed => self.completed += 1,
            TaskStatus::Failed => self.failed += 1,
            TaskStatus::DeadLetter => self.dead_letter += 1,
            TaskStatus::Cancelled => self.cancelled += 1,
        }
        self.total += 1;
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WalStats {
    pub epoch: u32,
    pub durable_lsn: String,
    pub next_lsn: String,
    pub unflushed_records: u64,
    pub oldest_unacked_ms: Option<u64>,
    pub poisoned: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SnapshotStats {
    pub last_round_at: Option<DateTime<Utc>>,
    pub dirty_shards: usize,
    pub oldest_dirty_lsn: Option<String>,
    pub shards_with_snapshot: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NodeStats {
    pub node_id: String,
    pub started_at: DateTime<Utc>,
    pub shards_owned: usize,
    pub shards_with_tasks: usize,
    pub tasks: TaskCounts,
    pub queues: Vec<String>,
    pub wal: WalStats,
    pub snapshots: SnapshotStats,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ShardStats {
    pub shard: u16,
    pub owner: Option<String>,
    pub epoch: u32,
    pub tasks: usize,
    pub pending: usize,
    pub running: usize,
    pub retry: usize,
    pub signals: usize,
    pub dead_letters: usize,
    pub shard_seq: u64,
    pub snapshot_seq: u64,
    pub snapshot_lsn: Option<String>,
    pub snapshot_at: Option<DateTime<Utc>>,
    pub records_since_snapshot: u64,
    pub dirty_since_lsn: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ShardDetail {
    #[serde(flatten)]
    pub stats: ShardStats,
    /// Tasks per queue held by this shard.
    pub queues: BTreeMap<String, TaskCounts>,
}

impl Engine {
    pub fn stats(&self) -> NodeStats {
        let mut tasks = TaskCounts::default();
        let mut queues = std::collections::BTreeSet::new();
        let mut shards_with_tasks = 0;
        let mut dirty_shards = 0;
        let mut oldest_dirty = None;
        let mut shards_with_snapshot = 0;
        for m in &self.inner.shards {
            let st = m.lock();
            if !st.tasks.is_empty() {
                shards_with_tasks += 1;
            }
            for t in st.tasks.values() {
                tasks.add(t.status);
                queues.insert(t.spec.queue_name.clone());
            }
            if st.records_since_snapshot > 0 {
                dirty_shards += 1;
                if let Some(d) = st.dirty_since_lsn {
                    oldest_dirty = Some(oldest_dirty.map_or(d, |o: valka_wal::Lsn| o.min(d)));
                }
            }
            if st.snapshot_seq > 0 {
                shards_with_snapshot += 1;
            }
        }
        let w = &self.inner.writer;
        NodeStats {
            node_id: self.inner.cfg.node_id.clone(),
            started_at: self.inner.started_at,
            shards_owned: ShardId::all().filter(|s| self.owns(*s)).count(),
            shards_with_tasks,
            tasks,
            queues: queues.into_iter().collect(),
            wal: WalStats {
                epoch: w.epoch(),
                durable_lsn: w.durable_lsn().to_string(),
                next_lsn: w.next_lsn().to_string(),
                unflushed_records: w.unflushed_records(),
                oldest_unacked_ms: w.oldest_unacked().map(|d| d.as_millis() as u64),
                poisoned: w.poisoned(),
            },
            snapshots: SnapshotStats {
                last_round_at: *self.inner.last_snapshot_round.lock(),
                dirty_shards,
                oldest_dirty_lsn: oldest_dirty.map(|l| l.to_string()),
                shards_with_snapshot,
            },
        }
    }

    /// One row per shard. ~4096 rows; cheap (one lock per shard, counts only).
    pub fn shard_stats(&self) -> Vec<ShardStats> {
        self.inner
            .shards
            .iter()
            .enumerate()
            .map(|(i, m)| self.shard_row(ShardId(i as u16), &m.lock()))
            .collect()
    }

    pub fn shard_detail(&self, shard: ShardId) -> Option<ShardDetail> {
        if shard.0 >= NUM_SHARDS {
            return None;
        }
        let st = self.inner.shards[shard.0 as usize].lock();
        let mut queues: BTreeMap<String, TaskCounts> = BTreeMap::new();
        for t in st.tasks.values() {
            queues
                .entry(t.spec.queue_name.clone())
                .or_default()
                .add(t.status);
        }
        Some(ShardDetail {
            stats: self.shard_row(shard, &st),
            queues,
        })
    }

    fn shard_row(&self, shard: ShardId, st: &ShardState) -> ShardStats {
        let owned = self.owns(shard);
        let mut pending = 0;
        let mut running = 0;
        let mut retry = 0;
        for t in st.tasks.values() {
            match t.status {
                TaskStatus::Pending => pending += 1,
                TaskStatus::Running => running += 1,
                TaskStatus::Retry => retry += 1,
                _ => {}
            }
        }
        ShardStats {
            shard: shard.0,
            owner: owned.then(|| self.inner.cfg.node_id.clone()),
            epoch: self.inner.writer.epoch(),
            tasks: st.tasks.len(),
            pending,
            running,
            retry,
            signals: st.signals.len(),
            dead_letters: st.dead_letters.len(),
            shard_seq: st.shard_seq,
            snapshot_seq: st.snapshot_seq,
            snapshot_lsn: (st.snapshot_seq > 0).then(|| st.snapshot_lsn.to_string()),
            snapshot_at: st.snapshot_at,
            records_since_snapshot: st.records_since_snapshot,
            dirty_since_lsn: st.dirty_since_lsn.map(|l| l.to_string()),
        }
    }
}
