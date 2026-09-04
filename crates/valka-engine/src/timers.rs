//! Timer wheel: a min-heap of `(fire_at, kind)`. Timer state is derived from task state
//! and is never persisted; firing a timer produces a WAL record like any other command.

use chrono::{DateTime, Utc};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimerKind {
    /// A RETRY or scheduled PENDING task becomes due.
    Promote { task_id: String },
    /// A RUNNING task's lease may have expired.
    LeaseExpiry { task_id: String, run_id: String },
    /// Periodic retention sweep for terminal tasks.
    Evict,
}

#[derive(Debug, Default)]
pub struct TimerWheel {
    heap: BinaryHeap<Reverse<(i64, TimerKind)>>,
}

impl TimerWheel {
    pub fn schedule(&mut self, at: DateTime<Utc>, kind: TimerKind) {
        self.heap.push(Reverse((at.timestamp_millis(), kind)));
    }

    /// Pop every timer due at or before `now`.
    pub fn due(&mut self, now: DateTime<Utc>) -> Vec<TimerKind> {
        let now_ms = now.timestamp_millis();
        let mut out = Vec::new();
        while let Some(Reverse((at, _))) = self.heap.peek() {
            if *at > now_ms {
                break;
            }
            let Reverse((_, kind)) = self.heap.pop().unwrap();
            out.push(kind);
        }
        out
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn next_fire(&self) -> Option<DateTime<Utc>> {
        self.heap
            .peek()
            .and_then(|Reverse((at, _))| DateTime::from_timestamp_millis(*at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_in_time_order_up_to_now() {
        let mut w = TimerWheel::default();
        let t0 = Utc::now();
        w.schedule(t0 + chrono::Duration::seconds(10), TimerKind::Evict);
        w.schedule(
            t0 + chrono::Duration::seconds(1),
            TimerKind::Promote {
                task_id: "b".into(),
            },
        );
        w.schedule(
            t0,
            TimerKind::Promote {
                task_id: "a".into(),
            },
        );
        let due = w.due(t0 + chrono::Duration::seconds(2));
        assert_eq!(due.len(), 2);
        assert_eq!(
            due[0],
            TimerKind::Promote {
                task_id: "a".into()
            }
        );
        assert_eq!(w.len(), 1);
        assert!(w.due(t0 + chrono::Duration::seconds(2)).is_empty());
    }
}
