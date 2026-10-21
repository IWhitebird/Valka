//! Regressions in the request layers: worker connections (a stuck or silent worker, a
//! reconnect noticed late), signal delivery while a task runs, forwarded commands retried
//! after a lost answer, message sizes, and partial cluster-wide answers.

use std::time::Duration;

use tokio::sync::{mpsc, watch};
use valka_cluster::{Command, Reply};
use valka_core::{PartitionId, ServerError, TaskStatus, WorkerId};
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_proto::{TaskResult, TaskSignal, WorkerResponse, worker_response};

use super::helpers::*;

fn handle(wid: &WorkerId, tx: mpsc::Sender<WorkerResponse>) -> WorkerHandle {
    WorkerHandle::new(
        wid.clone(),
        "w".to_string(),
        vec!["q".to_string()],
        1,
        tx,
        String::new(),
    )
}

fn filler() -> WorkerResponse {
    WorkerResponse { response: None }
}

/// The next message on a worker's stream that `pick` accepts.
async fn next_message(
    rx: &mut mpsc::Receiver<WorkerResponse>,
    pick: impl Fn(&worker_response::Response) -> bool,
) -> worker_response::Response {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for a worker message")
            .expect("worker stream closed");
        if let Some(r) = msg.response
            && pick(&r)
        {
            return r;
        }
    }
}

async fn next_signal(rx: &mut mpsc::Receiver<WorkerResponse>) -> TaskSignal {
    match next_message(rx, |r| {
        matches!(r, worker_response::Response::TaskSignal(_))
    })
    .await
    {
        worker_response::Response::TaskSignal(s) => s,
        _ => unreachable!(),
    }
}

fn signal_status(node: &TestNode, task_id: &str) -> String {
    node.engine.list_signals(task_id, None)[0].status.clone()
}

// ─── worker connections ─────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn a_worker_that_stops_reading_never_blocks_the_worker_map() {
    // Found in review: a cancel or signal for a worker whose stream was full waited for
    // room while holding the worker map's read guard, so every writer of that shard (the
    // worker's own heartbeats and results first) blocked a runtime thread for good.
    let node = TestNode::new().await;
    let wid = WorkerId::new();
    let (tx, _rx) = mpsc::channel(1);
    let mut h = handle(&wid, tx.clone());
    h.assign_task("t1".to_string());
    node.dispatcher.register_worker(h).await;
    tx.try_send(filler()).unwrap(); // the worker stopped reading: its stream is full

    let d = node.dispatcher.clone();
    let signal = tokio::spawn(async move {
        d.send_signal_to_worker(
            "t1",
            TaskSignal {
                signal_id: "s1".into(),
                task_id: "t1".into(),
                signal_name: "ping".into(),
                payload: String::new(),
                timestamp_ms: 0,
            },
        )
        .await
    });
    let d = node.dispatcher.clone();
    let cancel = tokio::spawn(async move { d.cancel_task_on_worker("t1").await });
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // Both wait for room in the stream, but not while holding the map.
    assert!(
        !matches!(
            node.dispatcher.workers().try_get_mut(wid.as_ref()),
            dashmap::try_result::TryResult::Locked
        ),
        "the worker map is locked by a pending delivery"
    );
    node.dispatcher
        .handle_heartbeat(&wid, valka_proto::Heartbeat::default())
        .await;
    // They give up instead of waiting forever, and say nothing was delivered.
    assert!(!signal.await.unwrap());
    assert!(!cancel.await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn a_silent_worker_is_disconnected_and_its_signals_reset() {
    // Found in review: the heartbeat checker removed a silent worker's handle itself, so
    // the deregistration that followed found nothing (delivered signals were never reset)
    // and the stream stayed open: a worker that was in fact alive kept heartbeating into
    // a connection that would never give it a task or acknowledge a result.
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    node.start(&t.id, &wid.0);
    let s = node.engine.send_signal(&t.id, "x", None).await.unwrap();
    node.engine.signal_delivered(&s.id);
    let (conn, closer) = {
        let mut h = node.dispatcher.workers().get_mut(wid.as_ref()).unwrap();
        h.assign_task(t.id.clone());
        h.last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(60);
        (h.conn_id, h.closer.clone())
    };
    settle().await;
    assert_eq!(signal_status(&node, &t.id), "DELIVERED");

    let (stop, stop_rx) = watch::channel(false);
    let (_checker, mut dead_rx) = node.dispatcher.start_heartbeat_checker(stop_rx);
    let dead = tokio::time::timeout(Duration::from_secs(10), dead_rx.recv())
        .await
        .expect("the silent worker is reported")
        .unwrap();
    assert_eq!(dead.worker_id, wid);
    assert_eq!(dead.conn_id, conn);
    assert!(
        closer.is_closed(),
        "its connection is closed, so a worker that is alive reconnects"
    );
    // What the server does with the report (server.rs).
    node.dispatcher
        .deregister_connection(&dead.worker_id, dead.conn_id)
        .await;
    settle().await;
    assert!(node.dispatcher.workers().is_empty());
    assert_eq!(
        signal_status(&node, &t.id),
        "PENDING",
        "back for redelivery"
    );
    let _ = stop.send(true);
}

#[tokio::test(start_paused = true)]
async fn a_late_cleanup_of_an_old_connection_leaves_the_reconnected_worker_alone() {
    // Found in review: cleanup went by worker id, and workers keep their id across
    // reconnects. A connection noticed gone only after the worker had reconnected removed
    // the new connection's handle and matching slots: connected, heartbeating, and never
    // given a task again.
    let node = TestNode::new().await;
    let wid = WorkerId::new();
    let (tx_a, _rx_a) = mpsc::channel(64);
    let a = handle(&wid, tx_a);
    let (conn_a, closer_a) = (a.conn_id, a.closer.clone());
    node.dispatcher.register_worker(a).await;
    let (d, w) = (node.dispatcher.clone(), wid.clone());
    let old_loop =
        tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".to_string()]).await });
    settle().await;

    // The worker reconnects before its old connection is noticed gone.
    let (tx_b, mut rx_b) = mpsc::channel(64);
    let b = handle(&wid, tx_b);
    let conn_b = b.conn_id;
    node.dispatcher.register_worker(b).await;
    assert!(closer_a.is_closed(), "the superseded connection is closed");
    tokio::time::timeout(Duration::from_secs(1), old_loop)
        .await
        .expect("the old connection's match loop ends")
        .unwrap();
    let (d, w) = (node.dispatcher.clone(), wid.clone());
    tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".to_string()]).await });
    settle().await;

    // Only now does the old connection's cleanup run.
    node.dispatcher.deregister_connection(&wid, conn_a).await;
    assert_eq!(
        node.dispatcher.workers().get(wid.as_ref()).unwrap().conn_id,
        conn_b
    );
    let t = node.create("q", "t").await;
    match next_message(&mut rx_b, |r| {
        matches!(r, worker_response::Response::TaskAssignment(_))
    })
    .await
    {
        worker_response::Response::TaskAssignment(a) => assert_eq!(a.task_id, t.id),
        _ => unreachable!(),
    }
}

#[tokio::test(start_paused = true)]
async fn a_task_already_in_a_closing_connections_slot_goes_back_at_once() {
    // Found in review: the match loop was aborted on disconnect and drain, dropping tasks
    // already sent into its slots; they sat out the 30 s offer lapse.
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    // Taken for this node's matching layer, as the feeder does.
    let taken = node.engine.take_pending("q", 1);
    assert_eq!(taken.len(), 1);
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let (d, w) = (node.dispatcher.clone(), wid.clone());
    let lp = tokio::spawn(async move { d.run_worker_match_loop(w, vec!["q".to_string()]).await });
    settle().await;

    // The task lands in the worker's slot, and in the same instant its connection ends.
    let envelope = valka_matching::MatchingSink::envelope(taken.into_iter().next().unwrap());
    node.matching
        .offer_task("q", PartitionId(0), envelope)
        .expect("a waiting slot takes it");
    let conn = node.dispatcher.workers().get(wid.as_ref()).unwrap().conn_id;
    node.dispatcher.deregister_connection(&wid, conn).await;
    tokio::time::timeout(Duration::from_secs(1), lp)
        .await
        .expect("the match loop ends")
        .unwrap();
    settle().await;
    assert_eq!(
        node.engine.pending_count("q"),
        1,
        "runnable again at once, not after the offer lapses"
    );
    assert_eq!(
        node.engine.get_task(&t.id).unwrap().status,
        TaskStatus::Pending
    );
}

// ─── signals while a task runs ──────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn signals_reset_by_a_disconnect_reach_the_reattached_run() {
    // Found in review: PENDING signals only travelled with a dispatch. A signal reset
    // when the worker's connection dropped never reached the run it reattached.
    let node = TestNode::new().await;
    let (wid, mut rx_a) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    let conn = {
        let mut h = node.dispatcher.workers().get_mut(wid.as_ref()).unwrap();
        h.assign_task(t.id.clone());
        h.conn_id
    };
    let (s, delivered) = node.api.send_signal(&t.id, "progress", None).await.unwrap();
    assert!(delivered);
    assert_eq!(next_signal(&mut rx_a).await.signal_id, s.id);

    // The connection drops before the worker acks.
    node.dispatcher.deregister_connection(&wid, conn).await;
    settle().await;
    assert_eq!(signal_status(&node, &t.id), "PENDING");

    // The worker reconnects, still running the task.
    let (tx_b, mut rx_b) = mpsc::channel(64);
    node.dispatcher
        .register_worker(handle(&wid, tx_b.clone()))
        .await;
    node.dispatcher
        .reattach_running(
            &wid,
            &[valka_proto::RunningTask {
                task_id: t.id.clone(),
                task_run_id: run,
                attempt_number: 1,
            }],
            &tx_b,
        )
        .await;
    assert_eq!(next_signal(&mut rx_b).await.signal_id, s.id);
    settle().await;
    assert_eq!(signal_status(&node, &t.id), "DELIVERED");
}

#[tokio::test(start_paused = true)]
async fn a_signal_sent_before_the_worker_holds_its_run_follows_it() {
    // Found in review: a signal created after the dispatch was recorded but before the
    // worker's node had the run found no worker and stayed PENDING for the whole run.
    let node = TestNode::new().await;
    let (wid, mut rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    node.start(&t.id, &wid.0);
    let (s, delivered) = node.api.send_signal(&t.id, "early", None).await.unwrap();
    assert!(!delivered, "the worker's node does not have the run yet");
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    assert_eq!(next_signal(&mut rx).await.signal_id, s.id);
    settle().await;
    assert_eq!(signal_status(&node, &t.id), "DELIVERED");
}

#[tokio::test(start_paused = true)]
async fn a_retried_dispatch_still_carries_the_pending_signals() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    node.engine.send_signal(&t.id, "early", None).await.unwrap();
    let shard = node.engine.shard_of(&t.id).unwrap();
    let run = valka_core::new_id_in_shard(shard).to_string();
    let first = node
        .engine
        .dispatch_to(&t.id, "w", "other-node", Some(&run))
        .unwrap();
    assert_eq!(first.pending_signals.len(), 1);
    // The first answer was lost: the caller retries with the same run id.
    let again = node
        .engine
        .dispatch_to(&t.id, "w", "other-node", Some(&run))
        .unwrap();
    assert_eq!(again.run_id, run);
    assert_eq!(again.pending_signals.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_worker_that_does_not_read_cannot_hold_up_a_signal_or_a_cancel() {
    // Found in review: the owner told the run's node inline, with nothing shorter than the
    // peer timeout, so a forwarded signal or cancel timed out at its sender and was sent
    // again.
    let node = TestNode::new().await;
    let wid = WorkerId::new();
    let (tx, mut rx) = mpsc::channel(1);
    node.dispatcher
        .register_worker(handle(&wid, tx.clone()))
        .await;
    let t = node.create("q", "t").await;
    node.start(&t.id, &wid.0);
    node.dispatcher
        .workers()
        .get_mut(wid.as_ref())
        .unwrap()
        .assign_task(t.id.clone());
    tx.try_send(filler()).unwrap();

    let started = tokio::time::Instant::now();
    let (s, delivered) = node.api.send_signal(&t.id, "x", None).await.unwrap();
    assert!(!delivered);
    assert!(
        started.elapsed() <= Duration::from_secs(2),
        "answered after {:?}",
        started.elapsed()
    );
    // The worker reads again: the signal follows.
    rx.recv().await.unwrap();
    assert_eq!(next_signal(&mut rx).await.signal_id, s.id);
    settle().await;
    assert_eq!(signal_status(&node, &t.id), "DELIVERED");

    // A cancel answers at once; the worker hears about it in the background.
    tx.try_send(filler()).unwrap();
    let started = tokio::time::Instant::now();
    let v = node.api.cancel_task(&t.id, "stop").await.unwrap();
    assert_eq!(v.status, TaskStatus::Cancelled);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "answered after {:?}",
        started.elapsed()
    );
}

// ─── forwarded commands retried after a lost answer ─────────────────

#[tokio::test(start_paused = true)]
async fn a_retried_cancel_answers_with_the_cancelled_task() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let cancel = |retry| Command::CancelTask {
        task_id: t.id.clone(),
        reason: "stop".into(),
        retry,
    };
    match node.api.execute_local(cancel(false)).await.unwrap() {
        Reply::Task(v) => assert_eq!(v.status, TaskStatus::Cancelled),
        r => panic!("{r:?}"),
    }
    // Sent again because the first answer was lost: the cancel's result, not an error.
    match node.api.execute_local(cancel(true)).await.unwrap() {
        Reply::Task(v) => assert_eq!(v.status, TaskStatus::Cancelled),
        r => panic!("{r:?}"),
    }
    // A user cancelling twice is still told the task is not cancellable.
    assert!(matches!(
        node.api.execute_local(cancel(false)).await,
        Err(ServerError::InvalidStatusTransition { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn a_retried_signal_creates_one_signal() {
    let node = TestNode::new().await;
    let t = node.create("q", "t").await;
    let send = |rid: &str| Command::SendSignal {
        task_id: t.id.clone(),
        name: "x".into(),
        payload: None,
        request_id: Some(rid.into()),
    };
    let id = |r: Reply| match r {
        Reply::SignalSent { signal, .. } => signal.id,
        r => panic!("{r:?}"),
    };
    let first = id(node.api.execute_local(send("req-1")).await.unwrap());
    let again = id(node.api.execute_local(send("req-1")).await.unwrap());
    assert_eq!(
        first, again,
        "the retry answers with the first attempt's signal"
    );
    assert_eq!(node.engine.list_signals(&t.id, None).len(), 1);
    let other = id(node.api.execute_local(send("req-2")).await.unwrap());
    assert_ne!(other, first);
    assert_eq!(node.engine.list_signals(&t.id, None).len(), 2);
}

// ─── results ────────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn a_plain_string_result_is_kept() {
    // Found in review: output that was not JSON (the Python SDK sends a returned str as
    // is) was dropped, and the result acknowledged as applied.
    let node = TestNode::new().await;
    let (wid, _rx) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    let run = node.start(&t.id, &wid.0);
    node.dispatcher
        .handle_task_result(
            &wid,
            TaskResult {
                task_id: t.id.clone(),
                task_run_id: run,
                success: true,
                retryable: false,
                output: "done".into(),
                error_message: String::new(),
            },
        )
        .await;
    let v = node.engine.get_task(&t.id).unwrap();
    assert_eq!(v.status, TaskStatus::Completed);
    assert_eq!(v.output, Some(serde_json::json!("done")));
}

// ─── message sizes ──────────────────────────────────────────────────

#[test]
fn the_message_limit_covers_what_a_task_may_carry() {
    let one_mib = 1024 * 1024;
    let max = valka_cluster::peer::max_message_bytes(one_mib);
    let checkpoints = valka_engine::MAX_CHECKPOINTS_PER_TASK * valka_engine::MAX_CHECKPOINT_BYTES;
    assert!(max > 2 * one_mib + checkpoints, "{max}");
    assert!(max > 4 * one_mib, "above tonic's default");
    assert_eq!(valka_cluster::peer::max_message_bytes(0), usize::MAX);
}

#[tokio::test(start_paused = true)]
async fn a_fetch_reply_stays_within_its_byte_budget() {
    // Found in review: a fetch reply carried up to 1000 tasks whatever their size; past
    // 4 MiB the fetching node could not decode it, the tasks stayed reserved for 30 s, and
    // the failures opened the circuit to a healthy owner.
    let node = TestNode::new().await;
    let big = "x".repeat(900 * 1024);
    for _ in 0..10 {
        let mut req = task_req("big", "t");
        req.input = Some(serde_json::json!(big));
        node.create_with(req).await;
    }
    assert_eq!(node.engine.pending_count("big"), 10);
    let fetched = match node
        .api
        .execute_local(Command::Fetch {
            queue: "big".into(),
            max: 10,
            wait_ms: 0,
        })
        .await
        .unwrap()
    {
        Reply::Fetched(t) => t,
        r => panic!("{r:?}"),
    };
    assert_eq!(fetched.len(), 4, "4 MiB of ~900 KiB tasks");
    assert_eq!(
        node.engine.pending_count("big"),
        6,
        "the rest are runnable again at once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_tasks_reach_workers_on_other_nodes() {
    use super::cluster_routing_tests::*;
    let c = cluster(2).await;
    // ~700 KiB each, created through node 0 (whose partitions take them).
    let big = "x".repeat(700 * 1024);
    let mut ids = std::collections::HashSet::new();
    for _ in 0..12 {
        let mut req = task_req("big", "t");
        req.input = Some(serde_json::json!(big));
        ids.insert(c.nodes[0].api.create_task(req).await.unwrap().id);
    }
    // A worker on node 1 with room for all of them: its fetches from node 0 ask for
    // several at once.
    let n1 = &c.nodes[1];
    let (wid, mut rx) = n1.register_worker(&["big"], 16).await;
    let (d, w) = (n1.dispatcher.clone(), wid.clone());
    tokio::spawn(async move { d.run_worker_match_loop(w, vec!["big".to_string()]).await });
    let mut got = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while got.len() < ids.len() {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(left, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("only {} of {} tasks arrived", got.len(), ids.len()))
            .unwrap();
        if let Some(worker_response::Response::TaskAssignment(a)) = msg.response {
            assert_eq!(a.input.len(), big.len() + 2);
            got.insert(a.task_id);
        }
    }
    assert_eq!(got, ids);
}

// ─── partial cluster-wide answers ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_that_does_not_answer_makes_shard_and_grpc_lists_partial() {
    use super::cluster_routing_tests::*;
    use axum::http::StatusCode;
    use tower::ServiceExt;
    let backing: std::sync::Arc<dyn object_store::ObjectStore> =
        std::sync::Arc::new(object_store::memory::InMemory::new());
    let (shutdown, _) = watch::channel(false);
    let (n1_listener, _) = watch::channel(false);
    let n0 = start_node(&backing, "n0", Some(TOKEN), Some(TOKEN), None, &shutdown).await;
    let n1 = start_node(&backing, "n1", Some(TOKEN), Some(TOKEN), None, &n1_listener).await;
    let c = Cluster {
        nodes: vec![n0, n1],
        shutdown,
    };
    c.converge().await;

    // Node 1 keeps its lease and shards but stops answering other nodes.
    let _ = n1_listener.send(true);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp = c.nodes[0]
        .router()
        .oneshot(get("/api/v1/cluster/shards"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let partial = resp.headers()["x-valka-partial"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        partial.starts_with("true") && partial.contains("n1"),
        "{partial}"
    );

    // gRPC lists say so too (in the response metadata).
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    tokio::spawn(valka_server::grpc::serve_grpc(
        addr,
        c.nodes[0].api.clone(),
        c.nodes[0].cluster_events.clone(),
        c.nodes[0].logs.clone(),
        valka_server::auth::ApiKeys::open(),
        c.shutdown.subscribe(),
    ));
    let mut client = loop {
        match valka_proto::api_service_client::ApiServiceClient::connect(format!("http://{addr}"))
            .await
        {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    let resp = client
        .list_tasks(valka_proto::ListTasksRequest::default())
        .await
        .unwrap();
    let partial = resp
        .metadata()
        .get("x-valka-partial")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        partial.starts_with("true") && partial.contains("n1"),
        "{partial}"
    );
}

// ─── the worker stream end to end ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_connection_ends_the_workers_stream() {
    use futures::StreamExt;
    let node = TestNode::new().await;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let (shutdown, shutdown_rx) = watch::channel(false);
    tokio::spawn(valka_server::grpc::serve_grpc(
        addr,
        node.api.clone(),
        node.cluster_events.clone(),
        node.logs.clone(),
        valka_server::auth::ApiKeys::open(),
        shutdown_rx,
    ));
    let mut client = loop {
        match valka_proto::worker_service_client::WorkerServiceClient::connect(format!(
            "http://{addr}"
        ))
        .await
        {
            Ok(c) => break c,
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    };
    let hello = |name: &str| valka_proto::WorkerRequest {
        request: Some(valka_proto::worker_request::Request::Hello(
            valka_proto::WorkerHello {
                worker_id: "w-e2e".into(),
                worker_name: name.into(),
                queues: vec!["q".into()],
                concurrency: 1,
                metadata: String::new(),
                running: Vec::new(),
            },
        )),
    };
    let registered = |name: &'static str| {
        let d = node.dispatcher.clone();
        async move {
            for _ in 0..100 {
                if d.workers()
                    .get("w-e2e")
                    .is_some_and(|h| h.worker_name == name)
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("worker {name} never registered");
        }
    };

    // First connection; the server's heartbeat checker finds it silent.
    let (req_a, rx) = mpsc::channel(8);
    req_a.send(hello("a")).await.unwrap();
    let mut inbound_a = client
        .session(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    registered("a").await;
    node.dispatcher
        .workers()
        .get_mut("w-e2e")
        .unwrap()
        .last_heartbeat = chrono::Utc::now() - chrono::Duration::seconds(60);
    let (_checker, mut dead_rx) = node
        .dispatcher
        .start_heartbeat_checker(shutdown.subscribe());
    let dead = tokio::time::timeout(Duration::from_secs(10), dead_rx.recv())
        .await
        .unwrap()
        .unwrap();
    node.dispatcher
        .deregister_connection(&dead.worker_id, dead.conn_id)
        .await;
    // The stream ends (the worker would reconnect) although the worker never hung up.
    let end = tokio::time::timeout(Duration::from_secs(3), inbound_a.next())
        .await
        .expect("the server ends a closed connection's stream");
    assert!(!matches!(end, Some(Ok(_))), "{end:?}");

    // The worker reconnects twice in a row; the first of the two is superseded at once.
    let (req_b, rx) = mpsc::channel(8);
    req_b.send(hello("b")).await.unwrap();
    let mut inbound_b = client
        .session(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    registered("b").await;
    let (req_c, rx) = mpsc::channel(8);
    req_c.send(hello("c")).await.unwrap();
    let mut inbound_c = client
        .session(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    registered("c").await;
    let end = tokio::time::timeout(Duration::from_secs(3), inbound_b.next())
        .await
        .expect("the superseded connection's stream ends");
    assert!(!matches!(end, Some(Ok(_))), "{end:?}");
    drop(req_b);
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The superseded connection's cleanup left the current one in place: it gets work.
    assert!(
        node.dispatcher
            .workers()
            .get("w-e2e")
            .is_some_and(|h| h.worker_name == "c")
    );
    let t = node.create("q", "t").await;
    let msg = tokio::time::timeout(Duration::from_secs(5), inbound_c.next())
        .await
        .expect("an assignment on the current connection")
        .unwrap()
        .unwrap();
    match msg.response {
        Some(worker_response::Response::TaskAssignment(a)) => assert_eq!(a.task_id, t.id),
        other => panic!("{other:?}"),
    }
    drop(req_a);
    drop(req_c);
    let _ = shutdown.send(true);
}

// ─── per-queue idempotency and per-worker heartbeats across nodes ───

#[tokio::test(start_paused = true)]
async fn an_idempotency_lookup_on_another_shard_is_per_queue() {
    // Keys are per queue: the lookup a keyed create makes in the key's old partition
    // (after a resize) must not answer with another queue's task.
    let node = TestNode::new().await;
    let mut req = task_req("a", "t");
    req.idempotency_key = Some("k1".into());
    let t = node.create_with(req).await;
    let shard = node.engine.shard_of(&t.id).unwrap();
    let find = |queue: Option<&str>| Command::FindIdempotent {
        probe_id: valka_core::new_id_in_shard(shard).to_string(),
        key: "k1".into(),
        queue: queue.map(str::to_string),
    };
    let found = |r: Reply| match r {
        Reply::MaybeTask(t) => t.map(|t| t.id),
        r => panic!("{r:?}"),
    };
    assert_eq!(
        found(node.api.execute_local(find(Some("a"))).await.unwrap()),
        Some(t.id.clone())
    );
    assert_eq!(
        found(node.api.execute_local(find(Some("b"))).await.unwrap()),
        None,
        "queue b has no task with key k1"
    );
    // An older node sends no queue: any queue's task with the key.
    assert_eq!(
        found(node.api.execute_local(find(None)).await.unwrap()),
        Some(t.id)
    );
}

#[tokio::test(start_paused = true)]
async fn a_heartbeat_extends_only_the_sending_workers_run() {
    // A worker still busy with a run that expired (the task went to another worker) must
    // not keep the newer run alive, locally or through another node.
    let node = TestNode::new().await;
    let (w1, _rx1) = node.register_worker(&["q"], 1).await;
    let (w2, _rx2) = node.register_worker(&["q"], 1).await;
    let t = node.create("q", "t").await;
    node.start(&t.id, &w1.0);
    let last = |node: &TestNode| node.engine.runs_for_task(&t.id).unwrap()[0].last_heartbeat;
    let started = last(&node);
    let beat = |ids: Vec<String>| valka_proto::Heartbeat {
        active_task_ids: ids,
        timestamp_ms: 0,
    };

    tokio::time::advance(Duration::from_secs(5)).await;
    node.dispatcher
        .handle_heartbeat(&w2, beat(vec![t.id.clone()]))
        .await;
    assert_eq!(last(&node), started, "another worker's heartbeat");
    // The same through the node command a worker's node sends to the owner.
    node.api
        .execute_local(Command::Heartbeat {
            task_ids: vec![t.id.clone()],
            worker_id: Some(w2.0.clone()),
        })
        .await
        .unwrap();
    assert_eq!(last(&node), started, "another worker's forwarded heartbeat");

    node.dispatcher
        .handle_heartbeat(&w1, beat(vec![t.id.clone()]))
        .await;
    let extended = last(&node);
    assert!(extended > started, "its own worker's heartbeat");
    tokio::time::advance(Duration::from_secs(5)).await;
    node.api
        .execute_local(Command::Heartbeat {
            task_ids: vec![t.id.clone()],
            worker_id: Some(w1.0.clone()),
        })
        .await
        .unwrap();
    assert!(
        last(&node) > extended,
        "its own worker's forwarded heartbeat"
    );
}
