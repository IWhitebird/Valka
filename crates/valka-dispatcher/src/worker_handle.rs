use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Notify, mpsc, watch};
use valka_core::WorkerId;
use valka_proto::WorkerResponse;

/// A one-way switch: once closed it stays closed, and every waiter (present or future)
/// sees it.
#[derive(Clone, Debug)]
pub struct Latch(Arc<watch::Sender<bool>>);

impl Latch {
    pub fn new() -> Self {
        Self(Arc::new(watch::channel(false).0))
    }

    pub fn close(&self) {
        self.0.send_replace(true);
    }

    pub fn is_closed(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once the latch is closed (at once if it already is).
    pub async fn closed(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }
}

impl Default for Latch {
    fn default() -> Self {
        Self::new()
    }
}

/// Source of connection ids: one per `WorkerHandle`, unique in this process.
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Represents a connected worker and its communication channel.
pub struct WorkerHandle {
    pub worker_id: WorkerId,
    /// This connection of the worker. A worker keeps its id across reconnects, so a
    /// connection that is cleaned up late must only remove its own handle, never the
    /// one its reconnect registered.
    pub conn_id: u64,
    pub worker_name: String,
    pub queues: Vec<String>,
    pub concurrency: i32,
    pub active_tasks: HashSet<String>,
    pub response_tx: mpsc::Sender<WorkerResponse>,
    pub last_heartbeat: DateTime<Utc>,
    pub connected_at: DateTime<Utc>,
    pub metadata: String,
    /// Notified when a task leaves `active_tasks`, so a full worker's match loop resumes.
    pub slot_freed: Arc<Notify>,
    /// The worker asked to drain: it finishes what it runs and gets nothing new.
    pub draining: bool,
    /// Closing it ends this connection: its stream handler stops and the response stream
    /// ends, so the worker reconnects.
    pub closer: Latch,
    /// Closed when this connection must get no new tasks (drain or disconnect): its match
    /// loop hands back what it holds and stops.
    pub stop_dispatch: Latch,
}

impl WorkerHandle {
    pub fn new(
        worker_id: WorkerId,
        worker_name: String,
        queues: Vec<String>,
        concurrency: i32,
        response_tx: mpsc::Sender<WorkerResponse>,
        metadata: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            worker_id,
            conn_id: NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed),
            worker_name,
            queues,
            concurrency,
            active_tasks: HashSet::new(),
            response_tx,
            last_heartbeat: now,
            connected_at: now,
            metadata,
            slot_freed: Arc::new(Notify::new()),
            draining: false,
            closer: Latch::new(),
            stop_dispatch: Latch::new(),
        }
    }

    /// Use `closer` (owned by the connection's transport) to end this connection.
    pub fn with_closer(mut self, closer: Latch) -> Self {
        self.closer = closer;
        self
    }

    /// The id this connection's slots carry in the matching service, so a connection's
    /// slots can be dropped without touching those of the same worker's next connection.
    pub fn slot_owner(&self) -> WorkerId {
        slot_owner(&self.worker_id, self.conn_id)
    }

    /// End this connection: no new tasks, and its stream closes.
    pub fn close(&self) {
        self.stop_dispatch.close();
        self.closer.close();
    }

    pub fn available_slots(&self) -> i32 {
        self.concurrency - self.active_tasks.len() as i32
    }

    pub fn is_idle(&self) -> bool {
        self.active_tasks.is_empty()
    }

    pub fn assign_task(&mut self, task_id: String) {
        self.active_tasks.insert(task_id);
    }

    pub fn complete_task(&mut self, task_id: &str) {
        if self.active_tasks.remove(task_id) {
            self.slot_freed.notify_one();
        }
    }

    pub fn update_heartbeat(&mut self) {
        self.last_heartbeat = Utc::now();
    }
}

/// See [`WorkerHandle::slot_owner`].
pub fn slot_owner(worker_id: &WorkerId, conn_id: u64) -> WorkerId {
    WorkerId(format!("{}#{conn_id}", worker_id.0))
}
