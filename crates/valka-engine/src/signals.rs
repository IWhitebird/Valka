//! Task signals: create, deliver, acknowledge, reset, list.

use serde_json::Value;
use valka_core::{ServerError, shard_of_task_id};
use valka_wal::WalRecord;

use crate::engine::Engine;
use crate::state::SignalStatus;
use crate::view::SignalView;

impl Engine {
    pub async fn send_signal(
        &self,
        task_id: &str,
        name: &str,
        payload: Option<Value>,
    ) -> Result<SignalView, ServerError> {
        let shard = self.shard_for(task_id)?;
        let signal_id = valka_core::shard::embed_shard(uuid::Uuid::now_v7(), shard).to_string();
        let (durable, _, _, view) = self.mutate(
            shard,
            |st| {
                let t = st
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| ServerError::TaskNotFound(task_id.into()))?;
                if t.is_terminal() {
                    return Err(ServerError::InvalidStatusTransition {
                        from: t.status.as_str().into(),
                        to: "SIGNAL".into(),
                    });
                }
                Ok((
                    WalRecord::SignalCreated {
                        signal_id: signal_id.clone(),
                        task_id: task_id.into(),
                        signal_name: name.into(),
                        payload: payload.clone(),
                    },
                    (),
                ))
            },
            |st| st.signals.get(&signal_id).map(|s| s.view()),
        )?;
        durable.wait().await?;
        view.ok_or_else(|| ServerError::Internal("signal vanished".into()))
    }

    pub fn signal_delivered(&self, signal_id: &str) {
        self.signal_transition(
            signal_id,
            |task_id| WalRecord::SignalDelivered {
                signal_id: signal_id.into(),
                task_id,
            },
            SignalStatus::Pending,
        );
    }

    pub fn signal_acked(&self, signal_id: &str) {
        self.signal_transition(
            signal_id,
            |task_id| WalRecord::SignalAcked {
                signal_id: signal_id.into(),
                task_id,
            },
            SignalStatus::Delivered,
        );
    }

    pub(crate) fn signal_transition(
        &self,
        signal_id: &str,
        make: impl Fn(String) -> WalRecord,
        expect: SignalStatus,
    ) {
        let Some(shard) = shard_of_task_id(signal_id) else {
            return;
        };
        let res = self.mutate(
            shard,
            |st| {
                let s = st
                    .signals
                    .get(signal_id)
                    .ok_or_else(|| ServerError::TaskNotFound(signal_id.into()))?;
                if s.status != expect {
                    return Err(ServerError::InvalidStatusTransition {
                        from: s.status.as_str().into(),
                        to: "".into(),
                    });
                }
                Ok((make(s.task_id.clone()), ()))
            },
            |_| (),
        );
        if let Ok((d, tr, _, _)) = res {
            self.spawn_after_durable(d, tr);
        }
    }

    /// Worker disconnected: delivered-but-unacked signals go back to PENDING.
    pub fn reset_signals(&self, task_id: &str) {
        let Some(shard) = shard_of_task_id(task_id) else {
            return;
        };
        let res = self.mutate(
            shard,
            |st| {
                let any = st
                    .signals
                    .values()
                    .any(|s| s.task_id == task_id && s.status == SignalStatus::Delivered);
                if !any {
                    return Err(ServerError::Internal("nothing to reset".into()));
                }
                Ok((
                    WalRecord::SignalsReset {
                        task_id: task_id.into(),
                    },
                    (),
                ))
            },
            |_| (),
        );
        if let Ok((d, tr, _, _)) = res {
            self.spawn_after_durable(d, tr);
        }
    }

    pub fn list_signals(&self, task_id: &str, status: Option<SignalStatus>) -> Vec<SignalView> {
        let Some(shard) = shard_of_task_id(task_id) else {
            return Vec::new();
        };
        let st = self.inner.shards[shard.0 as usize].lock();
        let mut v: Vec<SignalView> = st
            .signals
            .values()
            .filter(|s| s.task_id == task_id && !status.is_some_and(|x| x != s.status))
            .map(|s| s.view())
            .collect();
        v.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        v
    }

    pub fn pending_signals(&self, task_id: &str) -> Vec<SignalView> {
        self.list_signals(task_id, Some(SignalStatus::Pending))
    }
}
