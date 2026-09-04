use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use valka_core::TaskStatus;
use valka_wal::fault::{FaultConfig, faulty_over};
use valka_wal::{FailureOutcome, Store, reader};

use crate::clock::{Clock, TokioClock};
use crate::engine::{CreateTask, Engine, EngineConfig};
use crate::sink::{DispatchableTask, OfferOutcome, TaskSink};
use crate::state::SignalStatus;

fn create(queue: &str) -> CreateTask {
    CreateTask {
        queue_name: queue.into(),
        task_name: "job".into(),
        input: Some(serde_json::json!({"n": 1})),
        priority: 0,
        max_retries: 3,
        timeout_seconds: 30,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    }
}

async fn open(store: &Store) -> Engine {
    Engine::open(store.clone(), EngineConfig::for_tests("node-a"))
        .await
        .unwrap()
}

/// Records offers; accepts everything unless `reject` is set.
#[derive(Default)]
struct RecordingSink {
    offers: Mutex<Vec<DispatchableTask>>,
    reject: std::sync::atomic::AtomicBool,
}

impl TaskSink for RecordingSink {
    fn offer(&self, task: DispatchableTask) -> OfferOutcome {
        if self.reject.load(std::sync::atomic::Ordering::SeqCst) {
            return OfferOutcome::Rejected;
        }
        self.offers.lock().push(task);
        OfferOutcome::Buffered
    }
    fn capacity(&self, _q: &str) -> usize {
        if self.reject.load(std::sync::atomic::Ordering::SeqCst) {
            0
        } else {
            100
        }
    }
}

async fn settle() {
    // Let group commits + spawned after-durable hooks run.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test(start_paused = true)]
async fn create_get_list_and_idempotency() {
    let store = Store::memory();
    let e = open(&store).await;
    let a = e.create_task(create("q1")).await.unwrap();
    assert_eq!(a.status, TaskStatus::Pending);
    assert_eq!(e.get_task(&a.id).unwrap(), a);
    assert!(e.get_task("not-a-uuid").is_none());

    let mut idem = create("q1");
    idem.idempotency_key = Some("k".into());
    let b = e.create_task(idem.clone()).await.unwrap();
    let err = e.create_task(idem).await.unwrap_err();
    assert!(matches!(
        err,
        valka_core::ServerError::IdempotencyConflict(_)
    ));

    let _c = e.create_task(create("q2")).await.unwrap();
    assert_eq!(e.list_tasks(None, None, 50, 0).len(), 3);
    assert_eq!(e.list_tasks(Some("q1"), None, 50, 0).len(), 2);
    assert_eq!(
        e.list_tasks(None, Some(TaskStatus::Running), 50, 0).len(),
        0
    );
    let page = e.list_tasks(None, None, 2, 0);
    assert_eq!(page.len(), 2);
    assert_eq!(
        page[0].id,
        e.list_tasks(None, None, 50, 0)[0].id,
        "newest first"
    );
    assert_eq!(e.list_tasks(None, None, 2, 2).len(), 1);
    assert_eq!(e.queues(), vec!["q1".to_string(), "q2".to_string()]);
    assert_eq!(e.pending_count("q1"), 2);
    let _ = b;

    // Everything is in the bucket.
    e.sync().await.unwrap();
    let recs = reader::read_all(&store, "node-a", None).await.unwrap();
    assert_eq!(recs.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn dispatch_complete_and_runs() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w1").unwrap();
    assert_eq!(d.attempt, 1);
    assert_eq!(d.task.attempt_number, 1);
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Running);
    assert_eq!(e.pending_count("q"), 0);

    // Second dispatch of a running task is refused.
    assert!(e.dispatch(&t.id, "w2").is_err());

    let v = e
        .complete_run(&t.id, &d.run_id, Some(serde_json::json!({"ok": true})))
        .await
        .unwrap();
    assert_eq!(v.status, TaskStatus::Completed);
    assert_eq!(v.output, Some(serde_json::json!({"ok": true})));
    let runs = e.runs_for_task(&t.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "COMPLETED");
    assert_eq!(runs[0].worker_id, "w1");

    // Completing again is a precondition failure, not a crash.
    assert!(e.complete_run(&t.id, &d.run_id, None).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn retry_backoff_promotion_and_dead_letter() {
    let store = Store::memory();
    let sink = Arc::new(RecordingSink::default());
    let e = Engine::open_with(
        store.clone(),
        EngineConfig::for_tests("node-a"),
        TokioClock::new(),
        sink.clone(),
    )
    .await
    .unwrap();
    let mut req = create("q");
    req.max_retries = 2;
    let t = e.create_task(req).await.unwrap();
    settle().await;
    assert_eq!(sink.offers.lock().len(), 1, "hot path offers after durable");

    let d1 = e.dispatch(&t.id, "w").unwrap();
    let r = e.fail_run(&t.id, &d1.run_id, "boom", true).await.unwrap();
    assert!(matches!(r.outcome, FailureOutcome::Retry { .. }));
    let v = e.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Retry);
    assert_eq!(v.error_message.as_deref(), Some("boom"));
    assert!(v.scheduled_at.is_some(), "retry exposes next attempt time");
    assert_eq!(e.pending_count("q"), 0);

    // backoff for attempt 1 = 1s * 2^1 = 2s
    tokio::time::advance(Duration::from_millis(1500)).await;
    settle().await;
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Retry);
    tokio::time::advance(Duration::from_millis(700)).await;
    settle().await;
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Pending);
    assert_eq!(sink.offers.lock().len(), 2, "promoted task re-offered");
    assert_eq!(sink.offers.lock()[1].attempt_number, 2);

    let d2 = e.dispatch(&t.id, "w").unwrap();
    assert_eq!(d2.attempt, 2);
    let r = e
        .fail_run(&t.id, &d2.run_id, "boom again", true)
        .await
        .unwrap();
    assert_eq!(r.outcome, FailureOutcome::DeadLetter);
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::DeadLetter);
    let dls = e.list_dead_letters(None, 10, 0);
    assert_eq!(dls.len(), 1);
    assert_eq!(dls[0].task_id, t.id);
    assert_eq!(dls[0].attempt_count, 2);
    assert_eq!(e.list_dead_letters(Some("other"), 10, 0).len(), 0);
}

#[tokio::test(start_paused = true)]
async fn non_retryable_failure_is_terminal() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    let r = e
        .fail_run(&t.id, &d.run_id, "bad input", false)
        .await
        .unwrap();
    assert_eq!(r.outcome, FailureOutcome::Failed);
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Failed);
}

#[tokio::test(start_paused = true)]
async fn lease_expiry_retries_and_heartbeat_extends() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut req = create("q");
    req.timeout_seconds = 10; // lease = 10 + 30 grace = 40s
    let t = e.create_task(req).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();

    tokio::time::advance(Duration::from_secs(35)).await;
    e.heartbeat(std::slice::from_ref(&t.id)); // lease -> now + 60
    settle().await;
    tokio::time::advance(Duration::from_secs(30)).await; // t=65 > original 40
    settle().await;
    assert_eq!(
        e.get_task(&t.id).unwrap().status,
        TaskStatus::Running,
        "heartbeat kept it alive"
    );

    tokio::time::advance(Duration::from_secs(40)).await; // t=105 > 95
    settle().await;
    let v = e.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Retry);
    let runs = e.runs_for_task(&t.id).unwrap();
    assert_eq!(runs[0].status, "FAILED");
    assert_eq!(runs[0].error_message.as_deref(), Some("Lease expired"));
    let _ = d;

    // Heartbeat records were coalesced into the WAL.
    e.sync().await.unwrap();
    let recs = reader::read_all(&store, "node-a", None).await.unwrap();
    assert!(
        recs.iter()
            .any(|(_, r)| r.record.kind() == "lease_extended")
    );
    assert!(recs.iter().any(|(_, r)| r.record.kind() == "lease_expired"));
}

#[tokio::test(start_paused = true)]
async fn scheduled_task_waits_until_due() {
    let store = Store::memory();
    let sink = Arc::new(RecordingSink::default());
    let clock = TokioClock::new();
    let e = Engine::open_with(
        store.clone(),
        EngineConfig::for_tests("node-a"),
        clock.clone(),
        sink.clone(),
    )
    .await
    .unwrap();
    let mut req = create("q");
    req.scheduled_at = Some(clock.now() + chrono::Duration::seconds(30));
    let t = e.create_task(req).await.unwrap();
    settle().await;
    assert!(sink.offers.lock().is_empty());
    assert!(e.dispatch(&t.id, "w").is_err(), "not due yet");
    assert_eq!(e.pending_count("q"), 0);
    tokio::time::advance(Duration::from_secs(31)).await;
    settle().await;
    assert_eq!(sink.offers.lock().len(), 1);
    assert!(e.dispatch(&t.id, "w").is_ok());
}

#[tokio::test(start_paused = true)]
async fn cancel_running_reports_worker_and_blocks_stale_result() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w9").unwrap();
    let (v, worker) = e.cancel_task(&t.id, "user").await.unwrap();
    assert_eq!(v.status, TaskStatus::Cancelled);
    assert_eq!(worker.as_deref(), Some("w9"));
    assert!(e.complete_run(&t.id, &d.run_id, None).await.is_err());
    assert!(e.cancel_task(&t.id, "again").await.is_err());
    assert!(
        e.cancel_task("00000000-0000-7000-8000-000000000000", "x")
            .await
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn delete_and_clear() {
    let store = Store::memory();
    let e = open(&store).await;
    let a = e.create_task(create("q")).await.unwrap();
    let _b = e.create_task(create("q")).await.unwrap();
    assert!(e.delete_task(&a.id).await.unwrap());
    assert!(!e.delete_task(&a.id).await.unwrap());
    assert!(e.get_task(&a.id).is_none());
    assert_eq!(e.pending_count("q"), 1);
    assert_eq!(e.clear_all_tasks().await.unwrap(), 1);
    assert_eq!(e.list_tasks(None, None, 10, 0).len(), 0);
    assert_eq!(e.pending_count("q"), 0);
}

#[tokio::test(start_paused = true)]
async fn signal_lifecycle() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let s = e
        .send_signal(&t.id, "ping", Some(serde_json::json!({"x": 1})))
        .await
        .unwrap();
    assert_eq!(s.status, "PENDING");
    assert_eq!(e.pending_signals(&t.id).len(), 1);
    e.signal_delivered(&s.id);
    settle().await;
    assert_eq!(e.list_signals(&t.id, None)[0].status, "DELIVERED");
    assert!(e.pending_signals(&t.id).is_empty());
    e.reset_signals(&t.id);
    settle().await;
    assert_eq!(e.list_signals(&t.id, Some(SignalStatus::Pending)).len(), 1);
    e.signal_delivered(&s.id);
    settle().await;
    e.signal_acked(&s.id);
    settle().await;
    assert_eq!(e.list_signals(&t.id, None)[0].status, "ACKNOWLEDGED");

    let d = e.dispatch(&t.id, "w").unwrap();
    e.complete_run(&t.id, &d.run_id, None).await.unwrap();
    assert!(
        e.send_signal(&t.id, "late", None).await.is_err(),
        "terminal tasks reject signals"
    );
}

#[tokio::test(start_paused = true)]
async fn crash_and_recover_without_snapshot() {
    let store = Store::memory();
    let (pending_id, running_id, done_id, run_id);
    {
        let e = open(&store).await;
        let p = e.create_task(create("q")).await.unwrap();
        let r = e.create_task(create("q")).await.unwrap();
        let c = e.create_task(create("q")).await.unwrap();
        let dr = e.dispatch(&r.id, "w").unwrap();
        let dc = e.dispatch(&c.id, "w").unwrap();
        e.complete_run(&c.id, &dc.run_id, Some(serde_json::json!(42)))
            .await
            .unwrap();
        e.sync().await.unwrap();
        pending_id = p.id;
        running_id = r.id;
        done_id = c.id;
        run_id = dr.run_id;
        // drop without shutdown = crash
    }
    let e2 = open(&store).await;
    assert_eq!(
        e2.get_task(&pending_id).unwrap().status,
        TaskStatus::Pending
    );
    assert_eq!(
        e2.get_task(&running_id).unwrap().status,
        TaskStatus::Running
    );
    let done = e2.get_task(&done_id).unwrap();
    assert_eq!(done.status, TaskStatus::Completed);
    assert_eq!(done.output, Some(serde_json::json!(42)));
    assert_eq!(e2.pending_count("q"), 1);
    // Recovered RUNNING task got a grace lease and can still be completed by its worker.
    e2.complete_run(&running_id, &run_id, None).await.unwrap();
    // The new epoch is higher than the old one.
    assert_eq!(e2.durable_lsn().epoch, 2);
}

#[tokio::test(start_paused = true)]
async fn recovered_running_task_expires_after_grace() {
    let store = Store::memory();
    let id;
    {
        let e = open(&store).await;
        let t = e.create_task(create("q")).await.unwrap();
        e.dispatch(&t.id, "w").unwrap();
        e.sync().await.unwrap();
        id = t.id;
    }
    let e2 = open(&store).await;
    tokio::time::advance(Duration::from_secs(61)).await; // recovery_grace = 60
    settle().await;
    assert_eq!(e2.get_task(&id).unwrap().status, TaskStatus::Retry);
}

#[tokio::test(start_paused = true)]
async fn snapshot_truncates_wal_and_recovery_uses_it() {
    let store = Store::memory();
    let mut ids = Vec::new();
    {
        let e = open(&store).await;
        for _ in 0..20 {
            ids.push(e.create_task(create("q")).await.unwrap().id);
        }
        let d = e.dispatch(&ids[0], "w").unwrap();
        e.complete_run(&ids[0], &d.run_id, None).await.unwrap();
        e.snapshot_now().await;
        let segs = reader::list_segments(&store, "node-a", None).await.unwrap();
        assert!(
            segs.len() <= 1,
            "all covered segments truncated, got {}",
            segs.len()
        );
        // Tail after the snapshot.
        ids.push(e.create_task(create("q")).await.unwrap().id);
        e.sync().await.unwrap();
    }
    let e2 = open(&store).await;
    assert_eq!(e2.list_tasks(None, None, 100, 0).len(), 21);
    assert_eq!(e2.get_task(&ids[0]).unwrap().status, TaskStatus::Completed);
    assert_eq!(e2.pending_count("q"), 20);
    // A second snapshot round on the recovered engine keeps working.
    e2.snapshot_now().await;
    let e3 = open(&store).await;
    assert_eq!(e3.list_tasks(None, None, 100, 0).len(), 21);
}

#[tokio::test(start_paused = true)]
async fn shutdown_flushes_and_snapshots() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    e.shutdown().await.unwrap();
    let snaps = store.list("snapshots/").await.unwrap();
    assert_eq!(snaps.len(), 1);
    let e2 = open(&store).await;
    assert!(e2.get_task(&t.id).is_some());
}

#[tokio::test(start_paused = true)]
async fn feeder_drains_pending_when_sink_frees_up() {
    let store = Store::memory();
    let sink = Arc::new(RecordingSink::default());
    sink.reject.store(true, std::sync::atomic::Ordering::SeqCst);
    let e = Engine::open_with(
        store.clone(),
        EngineConfig::for_tests("node-a"),
        TokioClock::new(),
        sink.clone(),
    )
    .await
    .unwrap();
    for _ in 0..5 {
        e.create_task(create("q")).await.unwrap();
    }
    settle().await;
    assert_eq!(e.pending_count("q"), 5);
    assert!(sink.offers.lock().is_empty());
    sink.reject
        .store(false, std::sync::atomic::Ordering::SeqCst);
    settle().await;
    assert_eq!(sink.offers.lock().len(), 5);
    assert_eq!(e.pending_count("q"), 0);
    // unoffer puts a task back for the feeder.
    let id = sink.offers.lock()[0].task_id.clone();
    e.unoffer(&id);
    assert_eq!(e.pending_count("q"), 1);
    settle().await;
    assert_eq!(sink.offers.lock().len(), 6);
}

/// Random command sequence under injected faults, crash at the end, recover, and check
/// that every acknowledged operation is reflected and nothing else is.
#[tokio::test(start_paused = true)]
async fn crash_replay_property_under_faults() {
    use rand::{Rng, SeedableRng, rngs::StdRng};
    for seed in 0..12u64 {
        let backing: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::memory::InMemory::new());
        let (store, faults) = faulty_over(backing.clone(), seed);
        let mut cfg = EngineConfig::for_tests("node-a");
        cfg.wal.put_retries = 50;
        let e = Engine::open_with(
            store.clone(),
            cfg,
            TokioClock::new(),
            Arc::new(crate::sink::NoopSink),
        )
        .await
        .unwrap();
        // Recovery itself is not fault-tolerant by design (a failed open is retried by
        // the process supervisor); faults start once the engine is serving.
        faults.set_fail_rate(150);
        faults.set_lost_put_ack_rate(100);
        let mut rng = StdRng::seed_from_u64(seed);
        // acked expectations: task_id -> expected status
        let mut expected: std::collections::HashMap<String, TaskStatus> =
            std::collections::HashMap::new();
        let mut running: Vec<(String, String)> = Vec::new();
        for _ in 0..60 {
            match rng.random_range(0..10) {
                0..=4 => {
                    let t = e.create_task(create("q")).await.unwrap();
                    expected.insert(t.id, TaskStatus::Pending);
                }
                5..=6 => {
                    if let Some(id) = expected
                        .iter()
                        .find(|(_, s)| **s == TaskStatus::Pending)
                        .map(|(k, _)| k.clone())
                    {
                        let d = e.dispatch(&id, "w").unwrap();
                        expected.insert(id.clone(), TaskStatus::Running);
                        running.push((id, d.run_id));
                    }
                }
                7 => {
                    if !running.is_empty() {
                        let (id, run) = running.swap_remove(rng.random_range(0..running.len()));
                        e.complete_run(&id, &run, Some(serde_json::json!(1)))
                            .await
                            .unwrap();
                        expected.insert(id, TaskStatus::Completed);
                    }
                }
                8 => {
                    if let Some(id) = expected
                        .iter()
                        .find(|(_, s)| **s == TaskStatus::Pending)
                        .map(|(k, _)| k.clone())
                    {
                        e.cancel_task(&id, "rnd").await.unwrap();
                        expected.insert(id, TaskStatus::Cancelled);
                    }
                }
                _ => {
                    if rng.random_bool(0.3) {
                        e.snapshot_now().await;
                    }
                }
            }
        }
        // Dispatch records are async: make them durable before "crashing" so the
        // expectation table (which assumes they landed) is valid.
        e.sync().await.unwrap();
        drop(e);
        faults.set_fail_rate(0);
        faults.set_lost_put_ack_rate(0);

        let e2 = Engine::open_with(
            store.clone(),
            EngineConfig::for_tests("node-a"),
            TokioClock::new(),
            Arc::new(crate::sink::NoopSink),
        )
        .await
        .unwrap();
        assert_eq!(
            e2.list_tasks(None, None, 1000, 0).len(),
            expected.len(),
            "seed {seed}: task count"
        );
        for (id, status) in &expected {
            let got = e2
                .get_task(id)
                .unwrap_or_else(|| panic!("seed {seed}: task {id} lost"));
            assert_eq!(got.status, *status, "seed {seed}: task {id}");
        }
        let _ = Utc::now();
        let _: Arc<FaultConfig> = faults;
    }
}
