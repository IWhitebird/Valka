//! The internal RPC vocabulary: commands routed to a shard's owner, and queries every node
//! answers for its own shards. JSON inside the protobuf envelopes of `internal.proto`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use valka_core::{ServerError, TaskStatus};
use valka_engine::{
    CheckpointView, CreateTask, DeadLetterView, DispatchInfo, DispatchableTask, NodeStats,
    ReattachOutcome, ResultOutcome, RunResult, RunView, ShardDetail, ShardStats, SignalView,
    TaskView,
};

/// A command for another node: most are about one task and run on the owner of its
/// shard; a few are addressed to a node chosen by the caller (see [`Command::target`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// `id` was minted for its shard by the node that placed the task.
    CreateTask {
        id: String,
        req: CreateTask,
    },
    GetTask {
        task_id: String,
    },
    CancelTask {
        task_id: String,
        reason: String,
    },
    DeleteTask {
        task_id: String,
    },
    SendSignal {
        task_id: String,
        name: String,
        payload: Option<Value>,
    },
    ListSignals {
        task_id: String,
        status: Option<String>,
    },
    Runs {
        task_id: String,
    },
    Checkpoints {
        task_id: String,
    },
    /// Is a task with this idempotency key in `probe_id`'s shard? (dedupe across a
    /// partition resize: the key's old partition may live on another shard)
    FindIdempotent {
        probe_id: String,
        key: String,
    },
    /// Stats and per-queue task counts of `probe_id`'s shard (operator view).
    ShardDetail {
        probe_id: String,
    },

    // ── worker side, routed to the task's owner ──
    /// Dispatch a task (offered to `node_id` by a fetch) to one of that node's workers.
    /// `run_id` is minted by the caller, so a retry is idempotent.
    Dispatch {
        task_id: String,
        worker_id: String,
        node_id: String,
        run_id: String,
    },
    ReturnRun {
        task_id: String,
        run_id: String,
    },
    Unoffer {
        task_id: String,
    },
    ReportResult {
        task_id: String,
        run_id: String,
        result: RunResult,
    },
    Checkpoint {
        task_id: String,
        run_id: String,
        step: String,
        output: Value,
    },
    SignalDelivered {
        signal_id: String,
    },
    SignalAcked {
        signal_id: String,
    },
    ResetSignals {
        task_id: String,
    },
    /// A worker attached to `node_id` reconnected with this run in progress.
    Reattach {
        task_id: String,
        run_id: String,
        attempt: i32,
        worker_id: String,
        node_id: String,
    },

    // ── addressed to a node, not routed by shard ──
    /// Extend the leases of these running tasks (those the receiving node owns).
    Heartbeat {
        task_ids: Vec<String>,
    },
    /// Runnable tasks of `queue` for the caller's idle workers (long poll).
    Fetch {
        queue: String,
        max: usize,
        wait_ms: u64,
    },
    /// Tell the worker running `task_id` on the receiving node to stop.
    CancelOnWorker {
        task_id: String,
        reason: String,
    },
    /// Deliver a signal to the worker running its task on the receiving node.
    DeliverSignal {
        signal: SignalView,
    },
    /// Hand every shard over and stop (admin drain).
    Drain,
    /// A queue's partition count changed: drop the cached copy.
    InvalidateQueue {
        queue: String,
    },
}

/// Where a command goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target<'a> {
    /// The owner of this id's shard.
    Shard(&'a str),
    /// A node chosen by the caller.
    Node,
}

impl Command {
    pub fn target(&self) -> Target<'_> {
        match self {
            Command::CreateTask { id, .. } => Target::Shard(id),
            Command::GetTask { task_id }
            | Command::CancelTask { task_id, .. }
            | Command::DeleteTask { task_id }
            | Command::SendSignal { task_id, .. }
            | Command::ListSignals { task_id, .. }
            | Command::Runs { task_id }
            | Command::Checkpoints { task_id }
            | Command::Dispatch { task_id, .. }
            | Command::ReturnRun { task_id, .. }
            | Command::Unoffer { task_id }
            | Command::ReportResult { task_id, .. }
            | Command::Checkpoint { task_id, .. }
            | Command::Reattach { task_id, .. }
            | Command::ResetSignals { task_id } => Target::Shard(task_id),
            Command::FindIdempotent { probe_id, .. } | Command::ShardDetail { probe_id } => {
                Target::Shard(probe_id)
            }
            Command::SignalDelivered { signal_id } | Command::SignalAcked { signal_id } => {
                Target::Shard(signal_id)
            }
            Command::Heartbeat { .. }
            | Command::Fetch { .. }
            | Command::CancelOnWorker { .. }
            | Command::DeliverSignal { .. }
            | Command::Drain
            | Command::InvalidateQueue { .. } => Target::Node,
        }
    }

    /// The id whose embedded shard routes this command (empty for node commands).
    pub fn routing_id(&self) -> &str {
        match self.target() {
            Target::Shard(id) => id,
            Target::Node => "",
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Command::CreateTask { .. } => "create_task",
            Command::GetTask { .. } => "get_task",
            Command::CancelTask { .. } => "cancel_task",
            Command::DeleteTask { .. } => "delete_task",
            Command::SendSignal { .. } => "send_signal",
            Command::ListSignals { .. } => "list_signals",
            Command::Runs { .. } => "runs",
            Command::Checkpoints { .. } => "checkpoints",
            Command::FindIdempotent { .. } => "find_idempotent",
            Command::ShardDetail { .. } => "shard_detail",
            Command::Dispatch { .. } => "dispatch",
            Command::ReturnRun { .. } => "return_run",
            Command::Unoffer { .. } => "unoffer",
            Command::ReportResult { .. } => "report_result",
            Command::Checkpoint { .. } => "checkpoint",
            Command::SignalDelivered { .. } => "signal_delivered",
            Command::SignalAcked { .. } => "signal_acked",
            Command::ResetSignals { .. } => "reset_signals",
            Command::Reattach { .. } => "reattach",
            Command::Heartbeat { .. } => "heartbeat",
            Command::Fetch { .. } => "fetch",
            Command::CancelOnWorker { .. } => "cancel_on_worker",
            Command::DeliverSignal { .. } => "deliver_signal",
            Command::Drain => "drain",
            Command::InvalidateQueue { .. } => "invalidate_queue",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", content = "value", rename_all = "snake_case")]
pub enum Reply {
    Task(TaskView),
    MaybeTask(Option<TaskView>),
    Deleted(bool),
    SignalSent {
        signal: SignalView,
        delivered: bool,
    },
    Signals(Vec<SignalView>),
    Runs(Option<Vec<RunView>>),
    Checkpoints(Option<Vec<CheckpointView>>),
    ShardDetail(Box<ShardDetail>),
    Dispatched(Box<DispatchInfo>),
    Result(ResultOutcome),
    Checkpoint(CheckpointView),
    Fetched(Vec<DispatchableTask>),
    Reattached(ReattachOutcome),
    /// Whether a worker of the receiving node got the message.
    Delivered(bool),
    /// Task ids the receiving node does not own.
    NotOwned(Vec<String>),
    Done,
}

/// A connected worker, as listed by the node its stream is attached to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub id: String,
    pub node_id: String,
    pub name: String,
    pub queues: Vec<String>,
    pub concurrency: i32,
    pub active_tasks: usize,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub connected_at: chrono::DateTime<chrono::Utc>,
}

/// A question every node answers for the shards it serves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case")]
pub enum Query {
    /// Newest first; each node returns at most `limit` rows.
    ListTasks {
        queue: Option<String>,
        status: Option<TaskStatus>,
        limit: usize,
    },
    CountTasks {
        queue: Option<String>,
        status: Option<TaskStatus>,
    },
    DeadLetters {
        queue: Option<String>,
        limit: usize,
    },
    Workers,
    NodeStats,
    /// Rows for the shards this node serves.
    Shards,
    Queues,
    /// Drop every task in the shards this node serves.
    ClearTasks,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", content = "value", rename_all = "snake_case")]
pub enum ScatterReply {
    Tasks(Vec<TaskView>),
    Count(usize),
    DeadLetters(Vec<DeadLetterView>),
    Workers(Vec<WorkerInfo>),
    NodeStats(Box<NodeStats>),
    Shards(Vec<ShardStats>),
    Queues(Vec<String>),
    Cleared(usize),
}

/// What a forwarded call can fail with.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RpcError {
    /// The node does not own the shard. `hint` is who it thinks does.
    #[error("node does not own shard {shard} (hint: {hint:?})")]
    NotOwner { shard: u16, hint: Option<String> },
    /// The owner ran the command and it failed.
    #[error("{0}")]
    Server(ServerError),
    /// The call did not complete: connection, timeout, circuit open, bad response.
    #[error("transport: {0}")]
    Transport(String),
}

impl From<RpcError> for ServerError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::NotOwner { shard, .. } => ServerError::NotOwner(shard),
            RpcError::Server(e) => e,
            RpcError::Transport(m) => ServerError::Unavailable(m),
        }
    }
}

/// `RpcError` proto <-> Rust. The exact `ServerError` travels as JSON in `detail`.
pub fn error_to_proto(e: &ServerError, owner_hint: Option<String>) -> valka_proto::RpcError {
    let code = match e {
        ServerError::NotOwner(_) => "NOT_OWNER",
        ServerError::TaskNotFound(_)
        | ServerError::WorkerNotFound(_)
        | ServerError::QueueNotFound(_) => "NOT_FOUND",
        ServerError::InvalidStatusTransition { .. }
        | ServerError::TaskCancelled(_)
        | ServerError::LeaseExpired(_) => "INVALID_STATE",
        ServerError::IdempotencyConflict(_) => "CONFLICT",
        ServerError::InvalidArgument(_) => "BAD_REQUEST",
        ServerError::Unavailable(_) => "UNAVAILABLE",
        ServerError::Storage(_) | ServerError::Internal(_) => "INTERNAL",
    };
    valka_proto::RpcError {
        code: code.to_string(),
        message: e.to_string(),
        shard: match e {
            ServerError::NotOwner(s) => *s as u32,
            _ => 0,
        },
        owner_hint: owner_hint.unwrap_or_default(),
        detail: serde_json::to_vec(e).unwrap_or_default(),
    }
}

pub fn error_from_proto(e: valka_proto::RpcError) -> RpcError {
    if e.code == "NOT_OWNER" {
        return RpcError::NotOwner {
            shard: e.shard as u16,
            hint: (!e.owner_hint.is_empty()).then_some(e.owner_hint),
        };
    }
    match serde_json::from_slice::<ServerError>(&e.detail) {
        Ok(se) => RpcError::Server(se),
        Err(_) => RpcError::Server(ServerError::Internal(format!("{}: {}", e.code, e.message))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_as_json() {
        let c = Command::SendSignal {
            task_id: "t".into(),
            name: "ping".into(),
            payload: Some(serde_json::json!({"a": 1})),
        };
        let j = serde_json::to_vec(&c).unwrap();
        assert_eq!(serde_json::from_slice::<Command>(&j).unwrap(), c);
        assert_eq!(c.routing_id(), "t");
        let r = Reply::Deleted(true);
        let j = serde_json::to_vec(&r).unwrap();
        assert_eq!(serde_json::from_slice::<Reply>(&j).unwrap(), r);
    }

    #[test]
    fn errors_keep_their_exact_variant() {
        let e = ServerError::InvalidStatusTransition {
            from: "COMPLETED".into(),
            to: "CANCELLED".into(),
        };
        match error_from_proto(error_to_proto(&e, None)) {
            RpcError::Server(back) => assert_eq!(back, e),
            other => panic!("{other:?}"),
        }
        match error_from_proto(error_to_proto(&ServerError::NotOwner(7), Some("b".into()))) {
            RpcError::NotOwner { shard, hint } => {
                assert_eq!(shard, 7);
                assert_eq!(hint.as_deref(), Some("b"));
            }
            other => panic!("{other:?}"),
        }
    }
}
