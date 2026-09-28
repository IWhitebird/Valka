//! Step checkpoints: a running run records completed steps so a retry resumes after them.

use serde_json::Value;
use valka_core::{ServerError, shard_of_task_id};
use valka_wal::WalRecord;

use crate::engine::Engine;
use crate::view::CheckpointView;
use crate::write_path::ensure_running;

pub const MAX_CHECKPOINTS_PER_TASK: usize = 256;
pub const MAX_CHECKPOINT_BYTES: usize = 256 * 1024;
pub const MAX_STEP_NAME_LEN: usize = 256;

impl Engine {
    /// Record that `run_id` completed `step` with `output`. Returns once durable.
    pub async fn checkpoint(
        &self,
        task_id: &str,
        run_id: &str,
        step: &str,
        output: Value,
    ) -> Result<CheckpointView, ServerError> {
        if step.is_empty() || step.len() > MAX_STEP_NAME_LEN {
            return Err(ServerError::InvalidArgument(format!(
                "step name must be 1..={MAX_STEP_NAME_LEN} bytes"
            )));
        }
        let size = serde_json::to_vec(&output)
            .map_err(|e| ServerError::InvalidArgument(e.to_string()))?
            .len();
        if size > MAX_CHECKPOINT_BYTES {
            return Err(ServerError::InvalidArgument(format!(
                "checkpoint output is {size} bytes, limit is {MAX_CHECKPOINT_BYTES}"
            )));
        }
        let shard = self.shard_for(task_id)?;
        let (durable, _, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                ensure_running(t, run_id)?;
                if t.checkpoints.len() >= MAX_CHECKPOINTS_PER_TASK
                    && !t.checkpoints.iter().any(|c| c.step == step)
                {
                    return Err(ServerError::InvalidArgument(format!(
                        "task already has {MAX_CHECKPOINTS_PER_TASK} checkpoints"
                    )));
                }
                Ok((
                    WalRecord::TaskCheckpointed {
                        task_id: task_id.into(),
                        run_id: run_id.into(),
                        step: step.into(),
                        output: output.clone(),
                    },
                    (),
                ))
            },
            |st| {
                st.tasks
                    .get(task_id)
                    .and_then(|t| t.checkpoint_views().into_iter().find(|c| c.step == step))
            },
        )?;
        durable.wait().await?;
        view.ok_or_else(|| ServerError::Internal("checkpoint vanished after write".into()))
    }

    pub fn checkpoints_for_task(&self, task_id: &str) -> Option<Vec<CheckpointView>> {
        let shard = shard_of_task_id(task_id)?;
        self.inner.shards[shard.0 as usize]
            .lock()
            .tasks
            .get(task_id)
            .map(|t| t.checkpoint_views())
    }
}
