use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use valka_core::TaskStatus;
use valka_wal::fault::faulty_over;
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

/// Every record node-a wrote that is still in the bucket.
async fn node_records(store: &Store) -> Vec<(valka_wal::Lsn, valka_wal::Envelope)> {
    reader::read_node(store, "node-a").await.unwrap()
}

/// Snapshots that hold at least one record (acquisition snapshots of empty shards do not).
async fn snapshots_with_records(store: &Store) -> usize {
    valka_wal::snapshot::list_all(store)
        .await
        .unwrap()
        .values()
        .flatten()
        .filter(|k| k.seq > 0)
        .count()
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
    // The same key again answers with the first task (a retried create), not a second one.
    let again = e.create_task(idem).await.unwrap();
    assert_eq!(again.id, b.id);

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
    let recs = node_records(&store).await;
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
    let recs = node_records(&store).await;
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
    let loc = worker.expect("the run was in progress");
    assert_eq!(loc.worker_id, "w9");
    assert_eq!(loc.node_id, "node-a");
    assert_eq!(loc.run_id, d.run_id);
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
    // The new incarnation is higher than the old one, and the old log was split away.
    assert_eq!(e2.incarnation(), 2);
    assert!(
        reader::list_segments(&store, "node-a", 1)
            .await
            .unwrap()
            .is_empty()
    );
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
        let segs = reader::list_segments(&store, "node-a", e.incarnation())
            .await
            .unwrap();
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
    assert_eq!(snapshots_with_records(&store).await, 1);
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
    use rand::{RngExt, SeedableRng, rngs::StdRng};
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
    }
}

#[tokio::test(start_paused = true)]
async fn ownership_loss_poisons_writer_and_fails_acks() {
    // The zombie case: process A still runs while a replacement process for the same
    // node id takes its lease at a higher incarnation. A's next commit must be refused:
    // no ack, writer poisoned, later writes fail fast. B splits A's log and serves.
    let store = Store::memory();
    let cfg = EngineConfig::for_tests("a");
    let a = Engine::open(store.clone(), cfg.clone()).await.unwrap();
    let first = a.create_task(create("q")).await.unwrap();

    let b = Engine::open(store.clone(), cfg).await.unwrap();
    assert_eq!(b.incarnation(), 2);
    assert_eq!(b.get_task(&first.id).unwrap().status, TaskStatus::Pending);

    let err = a.create_task(create("q")).await.unwrap_err();
    assert!(
        matches!(err, valka_core::ServerError::Unavailable(_)),
        "{err}"
    );
    assert!(a.poisoned().is_some());
    assert!(a.create_task(create("q")).await.is_err());
    // B is unaffected and keeps serving.
    b.create_task(create("q")).await.unwrap();
    b.sync().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn concurrent_creates_are_batched_and_all_durable() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut handles = Vec::new();
    for i in 0..500u32 {
        let e = e.clone();
        handles.push(tokio::spawn(async move {
            e.create_task(create(&format!("q{}", i % 5)))
                .await
                .unwrap()
                .id
        }));
    }
    let ids: Vec<String> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(ids.len(), 500);
    assert_eq!(e.list_tasks(None, None, 1000, 0).len(), 500);
    for q in 0..5 {
        assert_eq!(e.pending_count(&format!("q{q}")), 100);
    }
    e.sync().await.unwrap();
    let segs = reader::list_segments(&store, "node-a", e.incarnation())
        .await
        .unwrap();
    assert!(
        segs.len() < 50,
        "group commit must batch: 500 records landed in {} segments",
        segs.len()
    );
    let recs = node_records(&store).await;
    assert_eq!(
        recs.iter()
            .filter(|(_, r)| r.record.kind() == "task_created")
            .count(),
        500
    );

    // And a fresh node sees exactly the same thing.
    drop(e);
    let e2 = open(&store).await;
    assert_eq!(e2.list_tasks(None, None, 1000, 0).len(), 500);
}

#[tokio::test(start_paused = true)]
async fn snapshot_during_concurrent_writes_loses_nothing() {
    // Writers keep appending while snapshot rounds run; recovery must reflect every ack.
    let store = Store::memory();
    let e = open(&store).await;
    let writer = {
        let e = e.clone();
        tokio::spawn(async move {
            let mut ids = Vec::new();
            for i in 0..200 {
                let t = e.create_task(create("q")).await.unwrap();
                if i % 3 == 0 {
                    let d = e.dispatch(&t.id, "w").unwrap();
                    e.complete_run(&t.id, &d.run_id, Some(serde_json::json!(i)))
                        .await
                        .unwrap();
                }
                ids.push(t.id);
            }
            ids
        })
    };
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(7)).await;
        e.snapshot_now().await;
    }
    let ids = writer.await.unwrap();
    let expected: Vec<TaskStatus> = ids
        .iter()
        .map(|id| e.get_task(id).unwrap().status)
        .collect();
    e.sync().await.unwrap();
    drop(e);

    let e2 = open(&store).await;
    for (id, status) in ids.iter().zip(expected) {
        let got = e2.get_task(id).unwrap_or_else(|| panic!("task {id} lost"));
        assert_eq!(got.status, status, "task {id}");
    }
    assert_eq!(e2.list_tasks(None, None, 1000, 0).len(), 200);
    // Runs survived too.
    let completed: Vec<_> = ids
        .iter()
        .filter(|id| e2.get_task(id).unwrap().status == TaskStatus::Completed)
        .collect();
    assert_eq!(completed.len(), 67);
    for id in completed {
        assert_eq!(e2.runs_for_task(id).unwrap().len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn corrupt_segment_fails_recovery_cleanly() {
    let store = Store::memory();
    {
        let e = open(&store).await;
        e.create_task(create("q")).await.unwrap();
        e.sync().await.unwrap();
    }
    let (_, key) = reader::list_node_segments(&store, "node-a")
        .await
        .unwrap()
        .pop()
        .unwrap();
    let (bytes, _) = store.get(&key).await.unwrap().unwrap();
    let mut bad = bytes.to_vec();
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;
    store.put(&key, bytes::Bytes::from(bad)).await.unwrap();

    let err = Engine::open(store.clone(), EngineConfig::for_tests("node-a"))
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, valka_core::ServerError::Storage(_)), "{err}");
}

#[tokio::test(start_paused = true)]
async fn idempotency_and_retention_survive_restart() {
    let store = Store::memory();
    let clock = TokioClock::new();
    let (done_id, live_id);
    {
        let e = Engine::open_with(
            store.clone(),
            EngineConfig::for_tests("node-a"),
            clock.clone(),
            Arc::new(crate::sink::NoopSink),
        )
        .await
        .unwrap();
        let mut req = create("q");
        req.idempotency_key = Some("order-1".into());
        let d = e.create_task(req.clone()).await.unwrap();
        let run = e.dispatch(&d.id, "w").unwrap();
        e.complete_run(&d.id, &run.run_id, None).await.unwrap();
        done_id = d.id;
        live_id = e.create_task(create("q")).await.unwrap().id;
        e.sync().await.unwrap();
    }
    // Past retention, the terminal task is evicted on recovery; the live one is kept.
    tokio::time::advance(Duration::from_secs(25 * 3600)).await;
    let e2 = Engine::open_with(
        store.clone(),
        EngineConfig::for_tests("node-a"),
        clock,
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap();
    assert!(e2.get_task(&done_id).is_none(), "evicted terminal task");
    assert_eq!(e2.get_task(&live_id).unwrap().status, TaskStatus::Pending);
    // The key is free again once its task is gone (24h idempotency window).
    let mut req = create("q");
    req.idempotency_key = Some("order-1".into());
    e2.create_task(req).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn lost_put_ack_retry_hits_create_conflict_and_succeeds() {
    let (store, faults) = faulty_over(Arc::new(object_store::memory::InMemory::new()), 9);
    let e = Engine::open_with(
        store.clone(),
        EngineConfig::for_tests("node-a"),
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap();
    faults.set_lost_put_ack_rate(1000);
    // Every PUT "fails" after landing; the retry sees AlreadyExists and treats it as success.
    let t = e.create_task(create("q")).await.unwrap();
    faults.set_lost_put_ack_rate(0);
    e.sync().await.unwrap();
    let recs = node_records(&store).await;
    assert_eq!(
        recs.iter()
            .filter(|(_, r)| r.record.task_id() == Some(t.id.as_str()))
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn node_and_shard_stats_reflect_state() {
    let store = Store::memory();
    let e = open(&store).await;
    let a = e.create_task(create("q1")).await.unwrap();
    let b = e.create_task(create("q2")).await.unwrap();
    let run = e.dispatch(&a.id, "w").unwrap();
    e.sync().await.unwrap();

    let s = e.stats();
    assert_eq!(s.node_id, "node-a");
    assert_eq!(s.shards_owned, 256);
    assert_eq!(s.num_shards, 256);
    assert!(s.serving);
    assert_eq!(s.tasks.total, 2);
    assert_eq!(s.tasks.pending, 1);
    assert_eq!(s.tasks.running, 1);
    assert_eq!(s.queues, vec!["q1".to_string(), "q2".to_string()]);
    assert_eq!(s.wal.unflushed_records, 0);
    assert!(s.wal.oldest_unacked_ms.is_none());
    assert!(s.wal.poisoned.is_none());
    assert!(s.snapshots.dirty_shards >= 1);
    assert!(s.snapshots.last_round_at.is_none());
    assert_eq!(s.snapshots.shards_with_snapshot, 0);

    let rows = e.shard_stats();
    assert_eq!(rows.len(), 256);
    assert!(
        rows.iter()
            .all(|r| r.phase == "serving" && r.generation == 1)
    );
    let with_tasks: Vec<_> = rows.iter().filter(|r| r.tasks > 0).collect();
    assert!(!with_tasks.is_empty() && with_tasks.len() <= 2);
    assert!(
        with_tasks
            .iter()
            .all(|r| r.owner.as_deref() == Some("node-a") && r.records_since_snapshot > 0)
    );

    let shard_a = e.shard_of(&a.id).unwrap();
    let d = e.shard_detail(shard_a).unwrap();
    assert_eq!(d.stats.shard, shard_a.0);
    assert_eq!(d.queues["q1"].running, 1);
    assert!(e.shard_detail(valka_core::ShardId(256)).is_none());

    e.snapshot_now().await;
    let s = e.stats();
    assert_eq!(s.snapshots.dirty_shards, 0);
    assert!(s.snapshots.last_round_at.is_some());
    let d = e.shard_detail(shard_a).unwrap();
    assert!(d.stats.snapshot_at.is_some());
    assert_eq!(d.stats.records_since_snapshot, 0);
    assert!(d.stats.snapshot_key.is_some());

    // After a crash the log is split into snapshots, and the claim snapshots each shard
    // again: the recovered shard is clean, at the next generation.
    e.complete_run(&a.id, &run.run_id, None).await.unwrap();
    e.sync().await.unwrap();
    drop(e);
    let e2 = open(&store).await;
    let d = e2.shard_detail(shard_a).unwrap();
    assert!(d.stats.snapshot_at.is_some());
    assert_eq!(d.stats.generation, 2);
    assert_eq!(d.stats.records_since_snapshot, 0);
    assert_eq!(e2.get_task(&a.id).unwrap().status, TaskStatus::Completed);
    let _ = b;
}

async fn promote_retry(e: &Engine, task_id: &str) {
    tokio::time::advance(Duration::from_secs(3)).await;
    settle().await;
    assert_eq!(e.get_task(task_id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test(start_paused = true)]
async fn checkpoints_carry_into_the_retry_dispatch() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();

    let d1 = e.dispatch(&t.id, "w1").unwrap();
    assert!(d1.checkpoints.is_empty());
    let c = e
        .checkpoint(
            &t.id,
            &d1.run_id,
            "download",
            serde_json::json!({"bytes": 10}),
        )
        .await
        .unwrap();
    assert_eq!(c.step, "download");
    assert_eq!(c.attempt_number, 1);
    assert_eq!(c.run_id, d1.run_id);
    e.checkpoint(&t.id, &d1.run_id, "parse", serde_json::json!([1, 2]))
        .await
        .unwrap();
    e.fail_run(&t.id, &d1.run_id, "enrich blew up", true)
        .await
        .unwrap();
    promote_retry(&e, &t.id).await;

    let d2 = e.dispatch(&t.id, "w2").unwrap();
    let steps: Vec<(&str, i32)> = d2
        .checkpoints
        .iter()
        .map(|c| (c.step.as_str(), c.attempt_number))
        .collect();
    assert_eq!(steps, vec![("download", 1), ("parse", 1)]);
    assert_eq!(d2.checkpoints[0].output, serde_json::json!({"bytes": 10}));

    e.checkpoint(&t.id, &d2.run_id, "enrich", serde_json::json!("ok"))
        .await
        .unwrap();
    e.checkpoint(&t.id, &d2.run_id, "parse", serde_json::json!([3]))
        .await
        .unwrap();
    e.complete_run(&t.id, &d2.run_id, None).await.unwrap();

    let all = e.checkpoints_for_task(&t.id).unwrap();
    let steps: Vec<(&str, i32)> = all
        .iter()
        .map(|c| (c.step.as_str(), c.attempt_number))
        .collect();
    assert_eq!(
        steps,
        vec![("download", 1), ("parse", 2), ("enrich", 2)],
        "re-checkpointing a step updates it in place"
    );
    assert_eq!(all[1].output, serde_json::json!([3]));

    e.sync().await.unwrap();
    let recs = node_records(&store).await;
    let written = recs
        .iter()
        .filter(|(_, r)| r.record.kind() == "task_checkpointed")
        .count();
    assert_eq!(written, 4);
}

#[tokio::test(start_paused = true)]
async fn checkpoint_is_fenced_to_the_current_running_run() {
    use valka_core::ServerError;
    let store = Store::memory();
    let e = open(&store).await;
    let mut req = create("q");
    req.timeout_seconds = 5;
    let t = e.create_task(req).await.unwrap();

    let pending = e
        .checkpoint(&t.id, "no-run", "s", serde_json::Value::Null)
        .await;
    assert!(matches!(
        pending,
        Err(ServerError::InvalidStatusTransition { .. })
    ));

    let d1 = e.dispatch(&t.id, "w").unwrap();
    tokio::time::advance(Duration::from_secs(36)).await;
    settle().await;
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Retry);
    promote_retry(&e, &t.id).await;
    let d2 = e.dispatch(&t.id, "w").unwrap();

    let stale = e
        .checkpoint(&t.id, &d1.run_id, "s", serde_json::Value::Null)
        .await;
    assert!(stale.is_err(), "the expired run may not checkpoint");
    assert!(d2.checkpoints.is_empty());

    e.complete_run(&t.id, &d2.run_id, None).await.unwrap();
    let finished = e
        .checkpoint(&t.id, &d2.run_id, "s", serde_json::Value::Null)
        .await;
    assert!(finished.is_err());

    let c = e.create_task(create("q")).await.unwrap();
    let dc = e.dispatch(&c.id, "w").unwrap();
    e.cancel_task(&c.id, "user").await.unwrap();
    let cancelled = e
        .checkpoint(&c.id, &dc.run_id, "s", serde_json::Value::Null)
        .await;
    assert!(cancelled.is_err());

    assert!(matches!(
        e.checkpoint("not-a-task", "r", "s", serde_json::Value::Null)
            .await,
        Err(ServerError::TaskNotFound(_))
    ));
    assert!(e.checkpoints_for_task(&t.id).unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn checkpoint_limits_are_enforced() {
    use crate::checkpoints::{MAX_CHECKPOINT_BYTES, MAX_CHECKPOINTS_PER_TASK, MAX_STEP_NAME_LEN};
    use valka_core::ServerError;
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    let run = d.run_id.as_str();
    let invalid = |r: Result<crate::CheckpointView, ServerError>| {
        matches!(r, Err(ServerError::InvalidArgument(_)))
    };

    assert!(invalid(
        e.checkpoint(&t.id, run, "", serde_json::Value::Null).await
    ));
    let long_name = "s".repeat(MAX_STEP_NAME_LEN + 1);
    assert!(invalid(
        e.checkpoint(&t.id, run, &long_name, serde_json::Value::Null)
            .await
    ));
    let too_big = serde_json::Value::String("x".repeat(MAX_CHECKPOINT_BYTES));
    assert!(invalid(e.checkpoint(&t.id, run, "big", too_big).await));

    for i in 0..MAX_CHECKPOINTS_PER_TASK {
        e.checkpoint(&t.id, run, &format!("step-{i}"), serde_json::json!(i))
            .await
            .unwrap();
    }
    assert!(invalid(
        e.checkpoint(&t.id, run, "one-too-many", serde_json::Value::Null)
            .await
    ));
    e.checkpoint(&t.id, run, "step-0", serde_json::json!("updated"))
        .await
        .expect("updating an existing step is allowed at the limit");
    assert_eq!(
        e.checkpoints_for_task(&t.id).unwrap().len(),
        MAX_CHECKPOINTS_PER_TASK
    );
}

#[tokio::test(start_paused = true)]
async fn checkpoints_survive_crash_via_snapshot_and_wal_tail() {
    let store = Store::memory();
    let id;
    {
        let e = open(&store).await;
        let t = e.create_task(create("q")).await.unwrap();
        let d = e.dispatch(&t.id, "w").unwrap();
        e.checkpoint(&t.id, &d.run_id, "in-snapshot", serde_json::json!(1))
            .await
            .unwrap();
        e.snapshot_now().await;
        e.checkpoint(&t.id, &d.run_id, "in-wal-tail", serde_json::json!(2))
            .await
            .unwrap();
        id = t.id;
    }
    let e2 = open(&store).await;
    let steps: Vec<String> = e2
        .checkpoints_for_task(&id)
        .unwrap()
        .into_iter()
        .map(|c| c.step)
        .collect();
    assert_eq!(steps, vec!["in-snapshot", "in-wal-tail"]);

    tokio::time::advance(Duration::from_secs(61)).await;
    settle().await;
    assert_eq!(e2.get_task(&id).unwrap().status, TaskStatus::Retry);
    promote_retry(&e2, &id).await;
    let d = e2.dispatch(&id, "w").unwrap();
    assert_eq!(d.attempt, 2);
    assert_eq!(d.checkpoints.len(), 2);
    assert_eq!(d.checkpoints[1].output, serde_json::json!(2));
}

#[tokio::test(start_paused = true)]
async fn snapshot_never_captures_records_that_failed_to_become_durable() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let (store, faults) = faulty_over(backing, 7);
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.wal.put_retries = 0;
    let e = Engine::open_with(
        store.clone(),
        cfg,
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap();
    let kept = e.create_task(create("q")).await.unwrap();

    faults.set_puts_down(true);
    assert!(e.create_task(create("q")).await.is_err());
    faults.set_puts_down(false);

    e.snapshot_now().await;
    assert_eq!(
        snapshots_with_records(&store).await,
        0,
        "a snapshot was written although the WAL could not be synced"
    );
    drop(e);

    let e2 = open(&store).await;
    let tasks = e2.list_tasks(None, None, 10, 0);
    assert_eq!(tasks.len(), 1, "only the acked task survives a restart");
    assert_eq!(tasks[0].id, kept.id);
}

#[tokio::test(start_paused = true)]
async fn snapshot_round_spanning_many_batches_covers_every_dirty_shard() {
    let store = Store::memory();
    let mut ids = Vec::new();
    {
        let e = open(&store).await;
        // Tasks of one queue share its 16 partition shards: spread over many queues.
        for i in 0..300 {
            ids.push(e.create_task(create(&format!("q{i}"))).await.unwrap().id);
        }
        let dirty = e.shard_stats().iter().filter(|s| s.tasks > 0).count();
        assert!(
            dirty > 128,
            "tasks should span several snapshot batches, got {dirty}"
        );
        e.snapshot_now().await;
        let snaps = snapshots_with_records(&store).await;
        assert_eq!(snaps, dirty, "one snapshot per dirty shard");
        let segs = reader::list_segments(&store, "node-a", e.incarnation())
            .await
            .unwrap();
        assert!(
            segs.len() <= 1,
            "covered segments truncated, got {}",
            segs.len()
        );
    }
    let e2 = open(&store).await;
    assert_eq!(e2.list_tasks(None, None, 1000, 0).len(), 300);
    for id in &ids {
        assert!(e2.get_task(id).is_some());
    }
}

fn incompressible(len: usize) -> String {
    let mut out = String::with_capacity(len);
    while out.len() < len {
        out.push_str(&uuid::Uuid::now_v7().simple().to_string());
    }
    out
}

#[tokio::test(start_paused = true)]
async fn log_budget_forces_a_snapshot_round_before_the_interval() {
    let store = Store::memory();
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.wal.log_budget_bytes = 8 * 1024;
    cfg.wal.snapshot_after_records = u64::MAX;
    let e = Engine::open(store.clone(), cfg).await.unwrap();
    for _ in 0..20 {
        let mut req = create("q");
        req.input = Some(serde_json::json!({"blob": incompressible(2048)}));
        e.create_task(req).await.unwrap();
    }
    assert_eq!(snapshots_with_records(&store).await, 0);

    tokio::time::advance(Duration::from_millis(1500)).await;
    settle().await;
    assert!(
        snapshots_with_records(&store).await > 0,
        "the budget, not the hour-long interval, triggered the round"
    );
    let segs = reader::list_segments(&store, "node-a", e.incarnation())
        .await
        .unwrap();
    assert!(
        segs.len() <= 1,
        "covered segments truncated, got {}",
        segs.len()
    );

    let e2 = open(&store).await;
    assert_eq!(e2.list_tasks(None, None, 100, 0).len(), 20);
}

#[tokio::test(start_paused = true)]
async fn zero_log_budget_leaves_rounds_to_the_interval() {
    let store = Store::memory();
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.wal.log_budget_bytes = 0;
    let e = Engine::open(store.clone(), cfg).await.unwrap();
    for _ in 0..20 {
        let mut req = create("q");
        req.input = Some(serde_json::json!({"blob": incompressible(2048)}));
        e.create_task(req).await.unwrap();
    }
    tokio::time::advance(Duration::from_secs(5)).await;
    settle().await;
    assert_eq!(snapshots_with_records(&store).await, 0);
}

fn completed(v: serde_json::Value) -> crate::RunResult {
    crate::RunResult::Completed(Some(v))
}

fn failed(error: &str) -> crate::RunResult {
    crate::RunResult::Failed {
        error: error.into(),
        retryable: true,
    }
}

async fn records_of(store: &Store, kind: &str) -> usize {
    node_records(store)
        .await
        .iter()
        .filter(|(_, r)| r.record.kind() == kind)
        .count()
}

#[tokio::test(start_paused = true)]
async fn repeated_results_are_applied_once() {
    use crate::ResultOutcome::Applied;
    let store = Store::memory();
    let e = open(&store).await;

    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    assert_eq!(
        e.report_result(&t.id, &d.run_id, completed(serde_json::json!(1)))
            .await,
        Applied
    );
    assert_eq!(
        e.report_result(&t.id, &d.run_id, completed(serde_json::json!(1)))
            .await,
        Applied
    );
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Completed);

    let f = e.create_task(create("q")).await.unwrap();
    let df = e.dispatch(&f.id, "w").unwrap();
    assert_eq!(
        e.report_result(&f.id, &df.run_id, failed("boom")).await,
        Applied
    );
    assert_eq!(
        e.report_result(&f.id, &df.run_id, failed("boom")).await,
        Applied
    );
    let v = e.get_task(&f.id).unwrap();
    assert_eq!(v.status, TaskStatus::Retry);
    assert_eq!(
        v.attempt_count, 1,
        "a resent failure does not spend another attempt"
    );

    e.sync().await.unwrap();
    assert_eq!(records_of(&store, "run_completed").await, 1);
    assert_eq!(records_of(&store, "run_failed").await, 1);
}

#[tokio::test(start_paused = true)]
async fn results_for_runs_that_ended_otherwise_are_stale() {
    use crate::ResultOutcome::Stale;
    let store = Store::memory();
    let e = open(&store).await;

    let mut req = create("q");
    req.timeout_seconds = 5;
    let expired = e.create_task(req).await.unwrap();
    let de = e.dispatch(&expired.id, "w").unwrap();
    tokio::time::advance(Duration::from_secs(36)).await;
    settle().await;
    assert_eq!(e.get_task(&expired.id).unwrap().status, TaskStatus::Retry);
    assert_eq!(
        e.report_result(&expired.id, &de.run_id, completed(serde_json::json!(1)))
            .await,
        Stale,
        "a completion after lease expiry is not recorded"
    );
    assert_eq!(
        e.report_result(&expired.id, &de.run_id, failed("late"))
            .await,
        Stale
    );

    let cancelled = e.create_task(create("q")).await.unwrap();
    let dc = e.dispatch(&cancelled.id, "w").unwrap();
    e.cancel_task(&cancelled.id, "user").await.unwrap();
    assert_eq!(
        e.report_result(&cancelled.id, &dc.run_id, failed("x"))
            .await,
        Stale
    );

    let deleted = e.create_task(create("q")).await.unwrap();
    let dd = e.dispatch(&deleted.id, "w").unwrap();
    e.delete_task(&deleted.id).await.unwrap();
    assert_eq!(
        e.report_result(&deleted.id, &dd.run_id, completed(serde_json::json!(1)))
            .await,
        Stale
    );
    assert_eq!(
        e.get_task(&cancelled.id).unwrap().status,
        TaskStatus::Cancelled
    );
}

#[tokio::test(start_paused = true)]
async fn results_that_cannot_be_made_durable_are_retried() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let (store, faults) = faulty_over(backing, 9);
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.wal.put_retries = 0;
    let e = Engine::open_with(
        store.clone(),
        cfg,
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap();
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    faults.set_puts_down(true);
    assert_eq!(
        e.report_result(&t.id, &d.run_id, completed(serde_json::json!(1)))
            .await,
        crate::ResultOutcome::Retry
    );
    assert_eq!(
        e.report_result(&t.id, &d.run_id, completed(serde_json::json!(1)))
            .await,
        crate::ResultOutcome::Retry,
        "RAM already shows the run completed, but that is not durable: never answer Applied"
    );
}

#[tokio::test(start_paused = true)]
async fn a_create_whose_caller_goes_away_is_still_published_once_durable() {
    // Found by the chaos rig: a create forwarded by a node that was killed mid-request was
    // cancelled while waiting for durability. The record became durable, but the task was
    // never indexed or offered, so it never ran.
    let store = Store::memory();
    let e = open(&store).await;
    let sink = Arc::new(RecordingSink::default());
    e.set_sink(sink.clone());
    e.create_task(create("q")).await.unwrap();
    settle().await;
    assert_eq!(sink.offers.lock().len(), 1);
    // The caller gives up while the record waits for its group commit.
    let gave_up = tokio::time::timeout(Duration::from_millis(1), e.create_task(create("q"))).await;
    assert!(
        gave_up.is_err(),
        "the create was still waiting for durability"
    );
    settle().await;
    assert_eq!(
        sink.offers.lock().len(),
        2,
        "the durable task was offered although its caller was gone"
    );
}

#[tokio::test(start_paused = true)]
async fn a_local_offer_that_is_never_dispatched_lapses_back_to_pending() {
    // Found by the chaos rig: the owner's own feeder handed a task to its matching layer,
    // the dispatch failed (the node was briefly not serving), and the task stayed
    // "offered" forever: PENDING, out of the pending index, never run.
    let store = Store::memory();
    let e = open(&store).await;
    let sink = Arc::new(RecordingSink::default());
    e.set_sink(sink.clone());
    let t = e.create_task(create("q")).await.unwrap();
    settle().await;
    assert_eq!(sink.offers.lock().len(), 1, "offered to local matching");
    assert_eq!(e.pending_count("q"), 0);
    // No dispatch ever comes. Stop accepting so the lapse is visible in the index.
    sink.reject.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::advance(crate::pending::OFFER_TTL + Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(
        e.pending_count("q"),
        1,
        "the lost offer lapsed back to PENDING"
    );
    let again = e.take_pending("q", 10);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].task_id, t.id);
}

#[tokio::test(start_paused = true)]
async fn fetch_reserves_tasks_for_another_node_and_unused_reservations_lapse() {
    let store = Store::memory();
    let e = open(&store).await;
    let a = e.create_task(create("q")).await.unwrap();
    let b = e.create_task(create("q")).await.unwrap();
    settle().await;
    let got = e.fetch("q", 10, Duration::from_millis(10)).await;
    assert_eq!(got.len(), 2);
    assert_eq!(e.pending_count("q"), 0, "reserved for the fetching node");
    assert!(
        e.fetch("q", 10, Duration::from_millis(10)).await.is_empty(),
        "nobody else gets them"
    );
    // The fetching node dispatches one to its worker; the other is never used.
    let d = e.dispatch_to(&a.id, "remote-w", "node-b", None).unwrap();
    assert_eq!(
        e.runs_for_task(&a.id).unwrap()[0].assigned_node_id,
        "node-b"
    );
    assert_eq!(d.attempt, 1);
    tokio::time::advance(crate::pending::OFFER_TTL + Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(e.pending_count("q"), 1, "the unused reservation lapsed");
    assert_eq!(e.get_task(&b.id).unwrap().status, TaskStatus::Pending);

    // A fetch waits for a task to appear (long poll).
    let waiter = {
        let e = e.clone();
        tokio::spawn(async move { e.fetch("later", 1, Duration::from_secs(5)).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let c = e.create_task(create("later")).await.unwrap();
    let got = waiter.await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].task_id, c.id);
}

#[tokio::test(start_paused = true)]
async fn dispatch_with_a_run_id_is_idempotent_and_a_returned_run_spends_no_attempt() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let shard = e.shard_of(&t.id).unwrap();
    let run = valka_core::new_id_in_shard(shard).to_string();
    let first = e.dispatch_to(&t.id, "w", "node-b", Some(&run)).unwrap();
    let again = e.dispatch_to(&t.id, "w", "node-b", Some(&run)).unwrap();
    assert_eq!(
        first.run_id, again.run_id,
        "a retried dispatch is the same run"
    );
    assert_eq!(e.runs_for_task(&t.id).unwrap().len(), 1);
    assert!(
        e.dispatch_to(&t.id, "other", "node-c", None).is_err(),
        "a different dispatch is refused"
    );
    assert!(matches!(
        e.dispatch_to(
            &t.id,
            "w",
            "node-b",
            Some("00000000-0000-7000-8000-00000000ffff")
        ),
        Err(valka_core::ServerError::InvalidArgument(_))
    ));

    // The worker was gone before the assignment reached it.
    e.return_run(&t.id, &run).await.unwrap();
    let v = e.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Pending);
    assert_eq!(v.attempt_count, 0);
    assert_eq!(e.runs_for_task(&t.id).unwrap()[0].status, "RETURNED");
    let next = e.dispatch(&t.id, "w2").unwrap();
    assert_eq!(next.attempt, 1, "the returned run did not spend an attempt");
    // Signals sent before a run started ride on its dispatch.
    let s = e.create_task(create("q")).await.unwrap();
    e.send_signal(&s.id, "early", None).await.unwrap();
    let d = e.dispatch(&s.id, "w").unwrap();
    assert_eq!(d.pending_signals.len(), 1);
    assert_eq!(d.pending_signals[0].signal_name, "early");
}

#[tokio::test(start_paused = true)]
async fn reattach_reattaches_adopts_or_calls_a_run_stale() {
    use crate::ReattachOutcome::{Adopted, Reattached, Stale};
    let store = Store::memory();
    let e = open(&store).await;

    // A current run, reconnecting through another node: it moves there.
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    assert_eq!(
        e.reattach(&t.id, &d.run_id, d.attempt, "w", "node-b")
            .await
            .unwrap(),
        Reattached
    );
    settle().await;
    let loc = e.run_location(&t.id).unwrap();
    assert_eq!(
        (loc.worker_id.as_str(), loc.node_id.as_str()),
        ("w", "node-b")
    );

    // A run whose dispatch record was lost: the task is PENDING at that very attempt.
    let lost = e.create_task(create("q")).await.unwrap();
    let shard = e.shard_of(&lost.id).unwrap();
    let run = valka_core::new_id_in_shard(shard).to_string();
    assert_eq!(
        e.reattach(&lost.id, &run, 1, "w2", "node-c").await.unwrap(),
        Adopted
    );
    let v = e.get_task(&lost.id).unwrap();
    assert_eq!(v.status, TaskStatus::Running);
    assert_eq!(e.runs_for_task(&lost.id).unwrap()[0].id, run);
    // ...but not at another attempt, nor twice.
    let other = e.create_task(create("q")).await.unwrap();
    let r2 = valka_core::new_id_in_shard(e.shard_of(&other.id).unwrap()).to_string();
    assert_eq!(
        e.reattach(&other.id, &r2, 2, "w", "n").await.unwrap(),
        Stale
    );

    // Finished, cancelled, superseded or unknown runs are stale.
    e.complete_run(&t.id, &d.run_id, None).await.unwrap();
    assert_eq!(
        e.reattach(&t.id, &d.run_id, d.attempt, "w", "node-b")
            .await
            .unwrap(),
        Stale
    );
    e.cancel_task(&lost.id, "user").await.unwrap();
    assert_eq!(
        e.reattach(&lost.id, &run, 1, "w2", "node-c").await.unwrap(),
        Stale
    );
    let gone = valka_core::new_id_in_shard(shard).to_string();
    assert_eq!(e.reattach(&gone, &gone, 1, "w", "n").await.unwrap(), Stale);
}

/// Says it has room, then refuses every offer: a waiting worker whose slot went away
/// between the count and the offer.
struct OvercountingSink;

impl TaskSink for OvercountingSink {
    fn offer(&self, _task: DispatchableTask) -> OfferOutcome {
        OfferOutcome::Rejected
    }
    fn capacity(&self, _q: &str) -> usize {
        100
    }
}

#[test]
fn a_refused_offer_strands_nothing_and_the_feeder_does_not_spin() {
    // Found by the benchmark: the feeder stopped at the first refused offer and dropped
    // the rest of its batch (out of the index, marked offered, for the 30 s lapse); and
    // handing the refused task back woke the feeder again at once, a busy loop that
    // wedged the runtime (the WAL committer, even timers, never ran again). The scenario
    // runs on its own runtime so a wedged one fails this test instead of hanging it.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let seen = rt.block_on(async {
            let e = Engine::open_with(
                Store::memory(),
                EngineConfig::for_tests("node-a"),
                TokioClock::new(),
                Arc::new(OvercountingSink),
            )
            .await
            .unwrap();
            for _ in 0..10 {
                e.create_task(create("q")).await.unwrap();
            }
            // The feeder keeps taking the tasks and handing them back; between two rounds
            // all ten are in the index (a stranded one would be out of it for 30 s).
            let mut seen = 0;
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(5)).await;
                seen = seen.max(e.pending_count("q"));
                if seen == 10 {
                    break;
                }
            }
            // Writes still complete while the feeder keeps being refused.
            e.create_task(create("q")).await.unwrap();
            seen
        });
        let _ = tx.send(seen);
    });
    let seen = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the engine wedged: the feeder is spinning");
    assert_eq!(seen, 10, "every refused task is runnable again");
}

// ───────────── regression tests for the engine review (timers, snapshots, keys, …) ─────────────

async fn open_cfg(store: &Store, cfg: EngineConfig) -> Engine {
    Engine::open_with(
        store.clone(),
        cfg,
        TokioClock::new(),
        Arc::new(crate::sink::NoopSink),
    )
    .await
    .unwrap()
}

/// Advance paused time in small steps, letting loops and spawned hooks run in between.
async fn run_for(d: Duration) {
    let step = Duration::from_millis(250);
    let mut left = d;
    while !left.is_zero() {
        let s = step.min(left);
        tokio::time::advance(s).await;
        tokio::task::yield_now().await;
        left -= s;
    }
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn timers_refused_while_the_lease_is_stale_fire_once_it_renews() {
    // Found by review: a promote or lease-expiry timer that fired while the lease was too
    // old to serve (the bucket slow for a few seconds) was refused and dropped, so the
    // RETRY task never ran again and the RUNNING task of a dead worker never expired.
    let (store, faults) = faulty_over(Arc::new(object_store::memory::InMemory::new()), 4);
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.cluster.lease_interval_ms = 2_000;
    cfg.cluster.lease_timeout_ms = 10_000;
    cfg.cluster.lease_safety_margin_ms = 3_000; // serves for 7 s after a renewal
    cfg.scheduler.retry_base_delay_secs = 5; // first retry 10 s after the failure
    let e = open_cfg(&store, cfg).await;

    let retry = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&retry.id, "w").unwrap();
    let mut short = create("q");
    short.timeout_seconds = 0; // lease = 30 s grace
    let running = e.create_task(short).await.unwrap();
    e.dispatch(&running.id, "w").unwrap();
    e.fail_run(&retry.id, &d.run_id, "boom", true)
        .await
        .unwrap();
    e.sync().await.unwrap();

    // The bucket fails: renewals fail, the lease goes stale, and both timers come due.
    faults.set_fail_rate(1000);
    run_for(Duration::from_secs(40)).await;
    assert!(!e.is_serving());
    assert_eq!(e.get_task(&retry.id).unwrap().status, TaskStatus::Retry);
    assert_eq!(e.get_task(&running.id).unwrap().status, TaskStatus::Running);

    // The bucket is back: the next renewal serves again and the refused timers fire.
    faults.set_fail_rate(0);
    run_for(Duration::from_secs(6)).await;
    assert!(e.is_serving());
    assert_eq!(e.get_task(&retry.id).unwrap().status, TaskStatus::Pending);
    assert_eq!(e.get_task(&running.id).unwrap().status, TaskStatus::Retry);
}

#[tokio::test(start_paused = true)]
async fn timers_refused_while_a_release_is_in_flight_fire_after_it_rolls_back() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    e.fail_run(&t.id, &d.run_id, "boom", true).await.unwrap(); // retry in 2 s
    let shard = e.shard_of(&t.id).unwrap();

    // The promote comes due while the shard is being handed over...
    e.inner.shards[shard.index()].lock().phase = crate::state::ShardPhase::Releasing;
    run_for(Duration::from_secs(3)).await;
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Retry);
    // ...and the hand-over fails: the shard serves again, and the timer was kept.
    e.inner.shards[shard.index()].lock().phase = crate::state::ShardPhase::Serving;
    run_for(Duration::from_secs(2)).await;
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test(start_paused = true)]
async fn snapshot_rounds_run_one_at_a_time() {
    // Found by review: a round (shutdown's, or the admin's) that ran during another one saw
    // the other's captured shards as clean and truncated the log below them while their
    // snapshots were still being written; a stop then lost those records (and the next
    // start's split failed on the gap).
    let store = Store::memory();
    let e = open(&store).await;
    for _ in 0..5 {
        e.create_task(create("q")).await.unwrap();
    }
    e.sync().await.unwrap();
    let in_flight = e.inner.snapshot_round.lock().await;
    let round = {
        let e = e.clone();
        tokio::spawn(async move { e.snapshot_now().await })
    };
    let stop = {
        let e = e.clone();
        tokio::spawn(async move { e.shutdown().await })
    };
    run_for(Duration::from_secs(2)).await;
    assert!(!round.is_finished() && !stop.is_finished(), "they wait");
    assert_eq!(snapshots_with_records(&store).await, 0);
    drop(in_flight);
    round.await.unwrap();
    stop.await.unwrap().unwrap();
    assert!(snapshots_with_records(&store).await >= 1);
    let e2 = open(&store).await;
    assert_eq!(e2.list_tasks(None, None, 100, 0).len(), 5);
}

#[tokio::test(start_paused = true)]
async fn a_key_freed_by_eviction_keeps_its_new_task_through_a_crash() {
    // Found by review: eviction is not in the WAL, so replay over a snapshot that still
    // held the evicted task treated a later create reusing its key as a duplicate and
    // dropped it: an acknowledged task vanished after a crash.
    let store = Store::memory();
    let clock = TokioClock::new();
    let open_now = || {
        Engine::open_with(
            store.clone(),
            EngineConfig::for_tests("node-a"),
            clock.clone(),
            Arc::new(crate::sink::NoopSink),
        )
    };
    let mut keyed = create("q");
    keyed.idempotency_key = Some("order-1".into());
    let first = {
        let e = open_now().await.unwrap();
        let t = e.create_task(keyed.clone()).await.unwrap();
        let run = e.dispatch(&t.id, "w").unwrap();
        e.complete_run(&t.id, &run.run_id, None).await.unwrap();
        e.sync().await.unwrap();
        e.halt();
        t.id
    };
    tokio::time::advance(Duration::from_secs(25 * 3600)).await;
    // The restart's claim snapshots the shard (still holding the old task), then evicts
    // it; the key is free and a new task takes it. Then the node is killed.
    let second = {
        let e = open_now().await.unwrap();
        assert!(e.get_task(&first).is_none());
        let t = e.create_task(keyed.clone()).await.unwrap();
        assert_ne!(t.id, first);
        e.sync().await.unwrap();
        e.halt();
        t.id
    };
    let e = open_now().await.unwrap();
    let got = e
        .get_task(&second)
        .expect("the acknowledged task survives replay");
    assert_eq!(got.status, TaskStatus::Pending);
    assert!(e.get_task(&first).is_none());
    // And the key still answers with it.
    assert_eq!(e.create_task(keyed).await.unwrap().id, second);
}

#[tokio::test(start_paused = true)]
async fn idempotency_keys_are_per_queue_even_on_one_shard() {
    // Found by review: the shard's key map ignored the queue, so a key used by two queues
    // that landed on one shard answered the second create with the first queue's task.
    let store = Store::memory();
    let e = open(&store).await;
    let shard = valka_core::ShardId(7);
    let id = || valka_core::new_id_in_shard(shard).to_string();
    let keyed = |queue: &str| {
        let mut r = create(queue);
        r.idempotency_key = Some("order-1".into());
        r
    };
    let a = e.create_task_at(&id(), keyed("emails")).await.unwrap();
    let b = e.create_task_at(&id(), keyed("invoices")).await.unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(b.queue_name, "invoices");
    // The same queue and key again is a retried create.
    assert_eq!(
        e.create_task_at(&id(), keyed("emails")).await.unwrap().id,
        a.id
    );
    assert_eq!(
        e.find_idempotent(shard, "invoices", "order-1").unwrap().id,
        b.id
    );
    // Both survive a crash.
    e.sync().await.unwrap();
    e.halt();
    let e2 = open(&store).await;
    assert_eq!(
        e2.find_idempotent(shard, "emails", "order-1").unwrap().id,
        a.id
    );
    assert_eq!(
        e2.find_idempotent(shard, "invoices", "order-1").unwrap().id,
        b.id
    );
}

#[tokio::test(start_paused = true)]
async fn queue_names_are_validated_on_create() {
    let store = Store::memory();
    let e = open(&store).await;
    for bad in ["a/b", ".hidden", "with space", &"q".repeat(200)] {
        let err = e.create_task(create(bad)).await.unwrap_err();
        assert!(
            matches!(err, valka_core::ServerError::InvalidArgument(_)),
            "{bad}: {err}"
        );
        let id = valka_core::new_id_in_shard(valka_core::ShardId(1)).to_string();
        assert!(e.create_task_at(&id, create(bad)).await.is_err(), "{bad}");
    }
    e.create_task(create("emails.v2:eu@prod_1-a"))
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn retention_evicts_finished_tasks_and_their_dead_letters_without_a_restart() {
    // Found by review: the retention sweep's timer was never armed, so finished tasks (and
    // dead letters, which nothing ever removed) stayed in RAM until a restart.
    let store = Store::memory();
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.wal.completed_retention_secs = 120;
    let e = open_cfg(&store, cfg).await;
    let done = e.create_task(create("q")).await.unwrap();
    let run = e.dispatch(&done.id, "w").unwrap();
    e.complete_run(&done.id, &run.run_id, None).await.unwrap();
    let mut once = create("q");
    once.max_retries = 1;
    let dead = e.create_task(once).await.unwrap();
    let run = e.dispatch(&dead.id, "w").unwrap();
    e.fail_run(&dead.id, &run.run_id, "boom", true)
        .await
        .unwrap();
    assert_eq!(e.get_task(&dead.id).unwrap().status, TaskStatus::DeadLetter);
    assert_eq!(e.list_dead_letters(None, 10, 0).len(), 1);
    let live = e.create_task(create("q")).await.unwrap();

    run_for(Duration::from_secs(60)).await;
    assert!(e.get_task(&done.id).is_some(), "kept within retention");
    run_for(Duration::from_secs(150)).await;
    assert!(e.get_task(&done.id).is_none());
    assert!(e.get_task(&dead.id).is_none());
    assert!(e.list_dead_letters(None, 10, 0).is_empty());
    assert_eq!(e.get_task(&live.id).unwrap().status, TaskStatus::Pending);
}

#[tokio::test(start_paused = true)]
async fn deleting_a_task_removes_its_dead_letter_even_after_the_task_left_ram() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut once = create("q");
    once.max_retries = 1;
    let t = e.create_task(once).await.unwrap();
    let run = e.dispatch(&t.id, "w").unwrap();
    e.fail_run(&t.id, &run.run_id, "boom", true).await.unwrap();
    // The task leaves RAM (as an eviction of an older version would leave it) but its
    // dead letter stays.
    let shard = e.shard_of(&t.id).unwrap();
    e.inner.shards[shard.index()].lock().tasks.remove(&t.id);
    assert_eq!(e.list_dead_letters(None, 10, 0).len(), 1);
    assert!(e.delete_task(&t.id).await.unwrap());
    assert!(e.list_dead_letters(None, 10, 0).is_empty());
    assert!(!e.delete_task(&t.id).await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn list_pages_match_a_full_sort() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut all = Vec::new();
    for i in 0..60 {
        let q = if i % 3 == 0 { "a" } else { "b" };
        all.push(e.create_task(create(q)).await.unwrap());
        if i % 7 == 0 {
            tokio::time::advance(Duration::from_millis(3)).await;
        }
    }
    for t in all.iter().step_by(4) {
        let d = e.dispatch(&t.id, "w").unwrap();
        e.complete_run(&t.id, &d.run_id, None).await.unwrap();
    }
    let full = |queue: Option<&str>, status: Option<TaskStatus>| {
        let mut v: Vec<_> = all
            .iter()
            .map(|t| e.get_task(&t.id).unwrap())
            .filter(|t| {
                queue.is_none_or(|q| q == t.queue_name) && status.is_none_or(|s| s == t.status)
            })
            .collect();
        v.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
        v
    };
    for (queue, status) in [
        (None, None),
        (Some("a"), None),
        (None, Some(TaskStatus::Completed)),
        (Some("b"), Some(TaskStatus::Pending)),
    ] {
        let expected = full(queue, status);
        for (limit, offset) in [(10, 0), (7, 5), (100, 0), (5, 55), (0, 3), (3, 100)] {
            let got = e.list_tasks(queue, status, limit, offset);
            let want: Vec<_> = expected.iter().skip(offset).take(limit).cloned().collect();
            assert_eq!(
                got, want,
                "queue {queue:?} status {status:?} {limit}/{offset}"
            );
        }
    }

    // Dead letters page the same way.
    let mut dead = Vec::new();
    for _ in 0..8 {
        let mut once = create("a");
        once.max_retries = 1;
        let t = e.create_task(once).await.unwrap();
        let d = e.dispatch(&t.id, "w").unwrap();
        e.fail_run(&t.id, &d.run_id, "x", true).await.unwrap();
        dead.push(t.id);
        tokio::time::advance(Duration::from_millis(2)).await;
    }
    let page: Vec<String> = e
        .list_dead_letters(None, 3, 2)
        .into_iter()
        .map(|d| d.task_id)
        .collect();
    let want: Vec<String> = dead.iter().rev().skip(2).take(3).cloned().collect();
    assert_eq!(page, want);
}

#[tokio::test(start_paused = true)]
async fn clearing_shards_empties_the_runnable_index_of_every_queue() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut ids = Vec::new();
    for i in 0..300 {
        ids.push(
            e.create_task(create(if i % 2 == 0 { "a" } else { "b" }))
                .await
                .unwrap()
                .id,
        );
    }
    assert_eq!(e.pending_count("a") + e.pending_count("b"), 300);
    assert!(e.delete_task(&ids[0]).await.unwrap());
    assert_eq!(e.pending_count("a"), 149);
    assert_eq!(e.clear_all_tasks().await.unwrap(), 299);
    assert_eq!(e.pending_count("a"), 0);
    assert_eq!(e.pending_count("b"), 0);
    // The index keeps working afterwards.
    e.create_task(create("a")).await.unwrap();
    assert_eq!(e.pending_count("a"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_heartbeat_extends_only_the_runs_of_the_worker_sending_it() {
    let store = Store::memory();
    let e = open(&store).await;
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w2").unwrap();
    let lease = |e: &Engine| e.runs_for_task(&t.id).unwrap()[0].lease_expires_at;
    let before = lease(&e);
    tokio::time::advance(Duration::from_secs(20)).await;
    // A worker whose own run of this task expired earlier keeps heartbeating it.
    e.heartbeat_from("w1", std::slice::from_ref(&t.id));
    assert_eq!(lease(&e), before);
    e.heartbeat_from("w2", std::slice::from_ref(&t.id));
    assert!(lease(&e) > before);
    let _ = d;
}

#[tokio::test(start_paused = true)]
async fn an_ended_run_leaves_no_long_lived_lease_timer() {
    let store = Store::memory();
    let e = open(&store).await;
    let before = e.inner.timers.lock().len();
    let mut long = create("q");
    long.timeout_seconds = 86_400;
    let t = e.create_task(long).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    e.complete_run(&t.id, &d.run_id, None).await.unwrap();
    assert_eq!(e.inner.timers.lock().len(), before + 1);
    // Checked within the horizon, found ended, gone; not kept for the whole day.
    run_for(Duration::from_secs(
        crate::write_path::LEASE_CHECK_HORIZON_SECS as u64 + 1,
    ))
    .await;
    assert_eq!(e.inner.timers.lock().len(), before);
}

#[tokio::test(start_paused = true)]
async fn a_long_running_run_is_still_expired_at_its_lease_end() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut long = create("q");
    long.timeout_seconds = 300;
    let t = e.create_task(long).await.unwrap();
    e.dispatch(&t.id, "w").unwrap();
    run_for(Duration::from_secs(300)).await;
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Running);
    run_for(Duration::from_secs(31)).await; // 300 s timeout + 30 s grace
    assert_eq!(e.get_task(&t.id).unwrap().status, TaskStatus::Retry);
}

#[tokio::test(start_paused = true)]
async fn the_lowest_priority_sorts_last() {
    let store = Store::memory();
    let e = open(&store).await;
    let mut low = create("q");
    low.priority = i32::MIN;
    let low = e.create_task(low).await.unwrap();
    let mut high = create("q");
    high.priority = i32::MAX;
    let high = e.create_task(high).await.unwrap();
    let normal = e.create_task(create("q")).await.unwrap();
    let order: Vec<String> = e
        .take_pending("q", 10)
        .into_iter()
        .map(|t| t.task_id)
        .collect();
    assert_eq!(order, vec![high.id, normal.id, low.id]);
}

#[tokio::test(start_paused = true)]
async fn log_ingester_keeps_draining_while_the_bucket_is_slow() {
    // Found by review: the ingester wrote one chunk per run, one after another, inside its
    // receive loop; with a slow bucket the channel filled and the worker streams blocked
    // on it (and stopped reading heartbeats).
    use valka_wal::logstore::LogLine;
    let (store, faults) = faulty_over(Arc::new(object_store::memory::InMemory::new()), 5);
    faults.set_latency(Duration::from_millis(100));
    let ingester = crate::LogIngester::new(store, 10, Duration::from_millis(500));
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    let (stop, stop_rx) = tokio::sync::watch::channel(false);
    let running = tokio::spawn(ingester.clone().run(rx, stop_rx));
    let started = tokio::time::Instant::now();
    for i in 0..2000i64 {
        tx.send(LogLine {
            task_run_id: format!("run-{}", i % 200),
            timestamp_ms: i,
            level: "INFO".into(),
            message: format!("line {i}"),
            metadata: None,
        })
        .await
        .unwrap();
    }
    let took = started.elapsed();
    assert!(took < Duration::from_secs(3), "senders waited {took:?}");
    stop.send(true).unwrap(); // lines still queued are written too
    running.await.unwrap();
    faults.set_latency(Duration::ZERO);
    for r in 0..200 {
        let lines = ingester.read(&format!("run-{r}"), 1000).await;
        let ts: Vec<i64> = lines.iter().map(|l| l.timestamp_ms).collect();
        let want: Vec<i64> = (0..10).map(|k| r + 200 * k).collect();
        assert_eq!(ts, want, "run-{r}: every line, in order");
    }
}

#[tokio::test(start_paused = true)]
async fn an_oversized_result_is_a_definite_failure() {
    // Found by the code review: a result larger than the payload limit was stored as is,
    // and one too large for a gRPC message broke the worker's stream on every resend.
    use crate::ResultOutcome::Applied;
    let store = Store::memory();
    let mut cfg = EngineConfig::for_tests("node-a");
    cfg.limits.max_payload_bytes = 100;
    let e = Engine::open(store.clone(), cfg).await.unwrap();
    let t = e.create_task(create("q")).await.unwrap();
    let d = e.dispatch(&t.id, "w").unwrap();
    let big = completed(serde_json::json!("x".repeat(500)));
    assert_eq!(e.report_result(&t.id, &d.run_id, big).await, Applied);
    let big_again = completed(serde_json::json!("x".repeat(500)));
    assert_eq!(e.report_result(&t.id, &d.run_id, big_again).await, Applied);
    let v = e.get_task(&t.id).unwrap();
    assert_eq!(
        v.status,
        TaskStatus::Failed,
        "not retried: it would fail again"
    );
    assert!(
        v.error_message
            .unwrap()
            .contains("limits.max_payload_bytes")
    );
}
