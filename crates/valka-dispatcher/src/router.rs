//! Where the worker side of a task goes: the node that owns the task's shard. A worker is
//! attached to one node, but the tasks it runs may live on any node; everything it says
//! about a task (dispatch, result, heartbeat, checkpoint, signal ack) goes through a
//! `TaskRouter`. The server's implementation forwards to the owner; [`LocalRouter`] is the
//! engine of this node, for a single node and for tests.

use async_trait::async_trait;
use serde_json::Value;
use valka_core::ServerError;
use valka_engine::{
    CheckpointView, DispatchInfo, Engine, ReattachOutcome, ResultOutcome, RunResult,
};

#[async_trait]
pub trait TaskRouter: Send + Sync {
    /// Record the dispatch of a task offered to this node to one of its workers.
    async fn dispatch(&self, task_id: &str, worker_id: &str) -> Result<DispatchInfo, ServerError>;
    /// The dispatched run never reached its worker: back to PENDING, no attempt spent.
    async fn return_run(&self, task_id: &str, run_id: &str);
    /// A task offered to this node was not dispatched: make it runnable again.
    async fn unoffer(&self, task_id: &str);
    async fn report_result(&self, task_id: &str, run_id: &str, result: RunResult) -> ResultOutcome;
    /// Extend the leases of the runs `worker_id` holds of these tasks.
    async fn heartbeat(&self, worker_id: &str, task_ids: &[String]);
    async fn checkpoint(
        &self,
        task_id: &str,
        run_id: &str,
        step: &str,
        output: Value,
    ) -> Result<CheckpointView, ServerError>;
    async fn signal_delivered(&self, signal_id: &str);
    async fn signal_acked(&self, signal_id: &str);
    /// The worker running the task went away: undelivered signals go back to PENDING.
    async fn reset_signals(&self, task_id: &str);
    /// A (re)connecting worker still runs this run, attached to this node now.
    async fn reattach(
        &self,
        task_id: &str,
        run_id: &str,
        attempt: i32,
        worker_id: &str,
    ) -> Result<ReattachOutcome, ServerError>;
}

/// Every task is this node's own.
pub struct LocalRouter(pub Engine);

#[async_trait]
impl TaskRouter for LocalRouter {
    async fn dispatch(&self, task_id: &str, worker_id: &str) -> Result<DispatchInfo, ServerError> {
        self.0.dispatch(task_id, worker_id)
    }
    async fn return_run(&self, task_id: &str, run_id: &str) {
        let _ = self.0.return_run(task_id, run_id).await;
    }
    async fn unoffer(&self, task_id: &str) {
        self.0.unoffer(task_id);
    }
    async fn report_result(&self, task_id: &str, run_id: &str, result: RunResult) -> ResultOutcome {
        self.0.report_result(task_id, run_id, result).await
    }
    async fn heartbeat(&self, worker_id: &str, task_ids: &[String]) {
        self.0.heartbeat_from(worker_id, task_ids);
    }
    async fn checkpoint(
        &self,
        task_id: &str,
        run_id: &str,
        step: &str,
        output: Value,
    ) -> Result<CheckpointView, ServerError> {
        self.0.checkpoint(task_id, run_id, step, output).await
    }
    async fn signal_delivered(&self, signal_id: &str) {
        self.0.signal_delivered(signal_id);
    }
    async fn signal_acked(&self, signal_id: &str) {
        self.0.signal_acked(signal_id);
    }
    async fn reset_signals(&self, task_id: &str) {
        self.0.reset_signals(task_id);
    }
    async fn reattach(
        &self,
        task_id: &str,
        run_id: &str,
        attempt: i32,
        worker_id: &str,
    ) -> Result<ReattachOutcome, ServerError> {
        let node = self.0.node_id().to_string();
        self.0
            .reattach(task_id, run_id, attempt, worker_id, &node)
            .await
    }
}
