//! End-to-end: a real gRPC server on a random port, the real Rust SDK worker and client,
//! and a real crash/restart of the server on the same bucket. Real time, not paused.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use valka_sdk::{ValkaClient, ValkaWorker};
use valka_wal::Store;

use super::helpers::TestNode;

struct RunningServer {
    node: TestNode,
    addr: String,
    shutdown: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

async fn start_server(store: Store, node_id: &str, port: u16) -> RunningServer {
    start_server_with_keys(store, node_id, port, valka_server::auth::ApiKeys::open()).await
}

async fn start_server_with_keys(
    store: Store,
    node_id: &str,
    port: u16,
    keys: valka_server::auth::ApiKeys,
) -> RunningServer {
    let node = TestNode::on_store(store, node_id).await;
    let (shutdown, rx) = watch::channel(false);
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let (api, event_tx, logs) = (node.api.clone(), node.event_tx.clone(), node.logs.clone());
    let handle = tokio::spawn(async move {
        valka_server::grpc::serve_grpc(addr, api, event_tx, logs, keys, rx)
            .await
            .expect("grpc server");
    });
    // Wait for the port to accept connections.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    RunningServer {
        node,
        addr: format!("http://127.0.0.1:{port}"),
        shutdown,
        handle,
    }
}

async fn wait_status(
    client: &mut ValkaClient,
    id: &str,
    want: i32,
    timeout: Duration,
) -> valka_proto::TaskMeta {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let t = client.get_task(id).await.unwrap();
        if t.status == want {
            return t;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task {id} stuck in status {} (wanted {want})",
            t.status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn e2e_sdk_worker_runs_tasks_end_to_end() {
    let port = free_port().await;
    let server = start_server(Store::memory(), "e2e", port).await;
    let handled = Arc::new(AtomicUsize::new(0));

    let h = handled.clone();
    let worker = ValkaWorker::builder()
        .name("e2e-worker")
        .server_addr(&server.addr)
        .queues(&["math"])
        .concurrency(4)
        .handler(move |ctx| {
            let h = h.clone();
            async move {
                let input: serde_json::Value = ctx.input().unwrap_or(serde_json::json!({}));
                ctx.log("working").await;
                h.fetch_add(1, Ordering::SeqCst);
                if input["fail"] == true {
                    return Err("requested failure".to_string());
                }
                let n = input["n"].as_i64().unwrap_or(0);
                Ok(serde_json::json!({"double": n * 2}))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let mut ids = Vec::new();
    for n in 0..10 {
        let t = client
            .create_task("math", "double", Some(serde_json::json!({"n": n})))
            .await
            .unwrap();
        assert_eq!(t.status, 1, "created tasks are PENDING");
        ids.push(t.id);
    }
    for (n, id) in ids.iter().enumerate() {
        let t = wait_status(&mut client, id, 4, Duration::from_secs(10)).await;
        let out: serde_json::Value = serde_json::from_str(&t.output).unwrap();
        assert_eq!(out["double"], (n as i64) * 2);
        assert_eq!(t.attempt_count, 1);
    }
    assert_eq!(handled.load(Ordering::SeqCst), 10);

    // A failing task goes to RETRY on the first attempt (max_retries defaults to 3).
    let f = client
        .create_task("math", "double", Some(serde_json::json!({"fail": true})))
        .await
        .unwrap();
    let t = wait_status(&mut client, &f.id, 6, Duration::from_secs(10)).await;
    assert_eq!(t.error_message, "requested failure");

    // Logs written by the handler landed in the bucket for the run.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let runs = server.node.engine.runs_for_task(&ids[0]).unwrap();
    let lines = server.node.logs.read(&runs[0].id, 10).await;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].message, "working");

    // Everything the client was acked for is in the WAL.
    server.node.engine.sync().await.unwrap();
    let recs = valka_wal::reader::read_node(&server.node.store, "e2e")
        .await
        .unwrap();
    let created = recs
        .iter()
        .filter(|(_, r)| r.record.kind() == "task_created")
        .count();
    assert_eq!(created, 11);

    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_task).await;
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
}

#[tokio::test]
async fn e2e_server_crash_and_restart_worker_reconnects() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let store = Store::wrap(backing.clone(), "", true, "shared-memory");
    let port = free_port().await;
    let server = start_server(store.clone(), "crashy", port).await;

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    // Create tasks with NO worker connected: they park in the bucket as PENDING.
    let mut ids = Vec::new();
    for n in 0..5 {
        ids.push(
            client
                .create_task("q", "t", Some(serde_json::json!({"n": n})))
                .await
                .unwrap()
                .id,
        );
    }
    server.node.engine.sync().await.unwrap();

    // Crash: stop the server without engine shutdown (no snapshot).
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
    drop(server.node);
    drop(client);

    // Restart on the same bucket and port.
    let server = start_server(store.clone(), "crashy", port).await;
    assert_eq!(server.node.engine.list_tasks(None, None, 100, 0).len(), 5);
    // Recovered runnable tasks are either still in the engine's pending index or already
    // handed to the matching buffers waiting for a worker.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        server.node.engine.pending_count("q") + server.node.matching.buffered("q"),
        5
    );

    let worker = ValkaWorker::builder()
        .name("late-worker")
        .server_addr(&server.addr)
        .queues(&["q"])
        .concurrency(2)
        .handler(|ctx| async move {
            let input: serde_json::Value = ctx.input().unwrap();
            Ok(serde_json::json!({"n": input["n"]}))
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    for id in &ids {
        wait_status(&mut client, id, 4, Duration::from_secs(10)).await;
    }

    // Second restart with a snapshot this time, then verify state is identical.
    server.node.engine.shutdown().await.unwrap();
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_task).await;
    drop(server.node);

    let node = TestNode::on_store(store, "crashy").await;
    for id in &ids {
        assert_eq!(
            node.engine.get_task(id).unwrap().status,
            valka_core::TaskStatus::Completed
        );
    }
}

#[tokio::test]
async fn e2e_retry_resumes_after_the_last_checkpointed_step() {
    let port = free_port().await;
    let server = start_server(Store::memory(), "e2e-steps", port).await;
    let runs: Arc<[AtomicUsize; 3]> = Arc::new(Default::default());

    let counters = runs.clone();
    let worker = ValkaWorker::builder()
        .name("steps-worker")
        .server_addr(&server.addr)
        .queues(&["pipeline"])
        .handler(move |ctx| {
            let counters = counters.clone();
            async move {
                let c = &counters;
                let rows: u32 = ctx
                    .step("fetch", || async move {
                        c[0].fetch_add(1, Ordering::SeqCst);
                        Ok::<_, String>(3)
                    })
                    .await?;
                let doubled: u32 = ctx
                    .step("transform", || async move {
                        if c[1].fetch_add(1, Ordering::SeqCst) == 0 {
                            return Err("flaky transform".to_string());
                        }
                        Ok(rows * 2)
                    })
                    .await?;
                let saved: String = ctx
                    .step("store", || async move {
                        c[2].fetch_add(1, Ordering::SeqCst);
                        Ok::<_, String>(format!("stored {doubled}"))
                    })
                    .await?;
                Ok(serde_json::json!({"result": saved, "attempt": ctx.attempt_number}))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let id = client
        .create_task("pipeline", "etl", None)
        .await
        .unwrap()
        .id;
    let t = wait_status(&mut client, &id, 4, Duration::from_secs(15)).await;

    let out: serde_json::Value = serde_json::from_str(&t.output).unwrap();
    assert_eq!(out, serde_json::json!({"result": "stored 6", "attempt": 2}));
    assert_eq!(t.attempt_count, 2);
    let executions: Vec<usize> = runs.iter().map(|c| c.load(Ordering::SeqCst)).collect();
    assert_eq!(
        executions,
        vec![1, 2, 1],
        "fetch ran once, transform retried, store once"
    );

    let steps: Vec<(String, i32)> = server
        .node
        .engine
        .checkpoints_for_task(&id)
        .unwrap()
        .into_iter()
        .map(|c| (c.step, c.attempt_number))
        .collect();
    assert_eq!(
        steps,
        vec![
            ("fetch".to_string(), 1),
            ("transform".to_string(), 2),
            ("store".to_string(), 2)
        ]
    );

    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_task).await;
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
}

#[tokio::test]
async fn e2e_result_finished_while_server_down_is_resent_and_applied_once() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let store = Store::wrap(backing, "", true, "shared-memory");
    let port = free_port().await;
    let server = start_server(store.clone(), "resend", port).await;

    let runs = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let (r, gate) = (runs.clone(), release.clone());
    let worker = ValkaWorker::builder()
        .name("resend-worker")
        .server_addr(&server.addr)
        .queues(&["q"])
        .handler(move |_ctx| {
            let (r, gate) = (r.clone(), gate.clone());
            async move {
                r.fetch_add(1, Ordering::SeqCst);
                gate.notified().await;
                Ok(serde_json::json!({"done": true}))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let id = client.create_task("q", "t", None).await.unwrap().id;
    wait_status(&mut client, &id, 3, Duration::from_secs(10)).await;
    server.node.engine.sync().await.unwrap();

    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
    drop(server.node);
    drop(client);
    release.notify_one();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let server = start_server(store, "resend", port).await;
    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let t = wait_status(&mut client, &id, 4, Duration::from_secs(15)).await;
    assert_eq!(t.attempt_count, 1, "the task was not re-run");
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the handler ran exactly once"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&t.output).unwrap(),
        serde_json::json!({"done": true})
    );

    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_task).await;
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
}

#[tokio::test]
async fn e2e_graceful_worker_shutdown_delivers_in_flight_results() {
    let port = free_port().await;
    let server = start_server(Store::memory(), "drain", port).await;
    let started = Arc::new(tokio::sync::Notify::new());
    let s = started.clone();
    let worker = ValkaWorker::builder()
        .name("drain-worker")
        .server_addr(&server.addr)
        .queues(&["q"])
        .handler(move |_ctx| {
            let s = s.clone();
            async move {
                s.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
                Ok(serde_json::json!({"drained": true}))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let id = client.create_task("q", "t", None).await.unwrap().id;
    started.notified().await;
    stop.shutdown();

    let exited = tokio::time::timeout(Duration::from_secs(10), worker_task).await;
    assert!(
        matches!(exited, Ok(Ok(Ok(())))),
        "worker exits cleanly: {exited:?}"
    );
    let t = client.get_task(&id).await.unwrap();
    assert_eq!(
        t.status, 4,
        "the in-flight task completed before the worker left"
    );
    assert_eq!(t.attempt_count, 1);

    let other = client.create_task("q", "t", None).await.unwrap().id;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        client.get_task(&other).await.unwrap().status,
        1,
        "a drained worker receives no new tasks"
    );

    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
}

#[tokio::test]
async fn e2e_task_outliving_a_restart_still_logs_and_checkpoints() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let store = Store::wrap(backing, "", true, "shared-memory");
    let port = free_port().await;
    let server = start_server(store.clone(), "outlive", port).await;

    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let worker = ValkaWorker::builder()
        .name("outlive-worker")
        .server_addr(&server.addr)
        .queues(&["q"])
        .handler(move |ctx| {
            let gate = gate.clone();
            async move {
                gate.notified().await;
                ctx.log("after restart").await;
                ctx.checkpoint("after-restart", &1)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(serde_json::json!({}))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let id = client.create_task("q", "t", None).await.unwrap().id;
    wait_status(&mut client, &id, 3, Duration::from_secs(10)).await;
    server.node.engine.sync().await.unwrap();
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
    drop(server.node);
    drop(client);

    let server = start_server(store, "outlive", port).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while server.node.dispatcher.workers().is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker never reconnected"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    release.notify_one();

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let t = wait_status(&mut client, &id, 4, Duration::from_secs(15)).await;
    assert_eq!(t.attempt_count, 1);
    let steps: Vec<String> = server
        .node
        .engine
        .checkpoints_for_task(&id)
        .unwrap()
        .into_iter()
        .map(|c| c.step)
        .collect();
    assert_eq!(steps, vec!["after-restart"]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let run = &server.node.engine.runs_for_task(&id).unwrap()[0].id;
    let lines = server.node.logs.read(run, 10).await;
    assert_eq!(
        lines.len(),
        1,
        "the log written after the reconnect reached the server"
    );
    assert_eq!(lines[0].message, "after restart");

    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_task).await;
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
}

/// The server crashes after dispatching a task but before the dispatch record reached the
/// bucket. The restarted server sees the task PENDING; the worker, still running it,
/// reconnects and reports the run in its hello, and the server adopts it instead of
/// running the task again (PHASE2 §11.4).
#[tokio::test]
async fn e2e_reconnecting_worker_adopts_a_run_whose_dispatch_was_lost() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let (faulty, faults) = valka_wal::fault::faulty_over(backing.clone(), 5);
    let port = free_port().await;
    let server = start_server(faulty, "adopt", port).await;
    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let id = client.create_task("q", "t", None).await.unwrap().id;
    // From here nothing reaches the bucket: the dispatch record will be lost.
    faults.set_puts_down(true);

    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let (s, gate) = (started.clone(), release.clone());
    let worker = ValkaWorker::builder()
        .name("adopted-worker")
        .server_addr(&server.addr)
        .queues(&["q"])
        .concurrency(1)
        .handler(move |_ctx| {
            let (s, gate) = (s.clone(), gate.clone());
            async move {
                s.fetch_add(1, Ordering::SeqCst);
                gate.notified().await;
                Ok(serde_json::json!({"adopted": true}))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let worker_task = tokio::spawn(worker.run());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while started.load(Ordering::SeqCst) == 0 {
        assert!(tokio::time::Instant::now() < deadline, "task never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let run_on_worker = server.node.engine.runs_for_task(&id).unwrap()[0].id.clone();

    // Crash, bucket back, restart on the same bucket and port.
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
    server.node.engine.halt();
    drop(server.node);
    drop(client);
    faults.set_puts_down(false);
    let store = Store::wrap(backing, "", true, "shared-memory");
    let server = start_server(store, "adopt", port).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let runs = server.node.engine.runs_for_task(&id).unwrap_or_default();
        if runs.first().is_some_and(|r| r.id == run_on_worker) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the reconnecting worker's run was not adopted: {runs:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    release.notify_one();
    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let t = wait_status(&mut client, &id, 4, Duration::from_secs(15)).await;
    assert_eq!(t.attempt_count, 1);
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "the task ran exactly once"
    );
    assert_eq!(server.node.engine.runs_for_task(&id).unwrap().len(), 1);

    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), worker_task).await;
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
}

#[tokio::test]
async fn e2e_api_keys_protect_the_grpc_api() {
    let port = free_port().await;
    let server = start_server_with_keys(
        Store::memory(),
        "keys",
        port,
        valka_server::auth::ApiKeys::parse(Some("k1")),
    )
    .await;

    // Without the key: refused, and not retried (it would never succeed).
    let mut anon = ValkaClient::builder()
        .addr(&server.addr)
        .api_key("")
        .connect()
        .await
        .unwrap();
    match anon.create_task("q", "t", None).await.unwrap_err() {
        valka_sdk::SdkError::Grpc(s) => assert_eq!(s.code(), tonic::Code::Unauthenticated),
        other => panic!("expected Unauthenticated, got {other}"),
    }

    // With the key: the client creates, and a worker with the key runs the task, steps
    // (the unary Checkpoint call) included.
    let mut client = ValkaClient::builder()
        .addr(&server.addr)
        .api_key("k1")
        .connect()
        .await
        .unwrap();
    let t = client.create_task("q", "t", None).await.unwrap();
    let worker = ValkaWorker::builder()
        .server_addr(&server.addr)
        .api_key("k1")
        .queues(&["q"])
        .handler(|ctx| async move {
            let n: i64 = ctx.step("one", || async { Ok::<_, String>(1) }).await?;
            Ok(serde_json::json!({ "n": n }))
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let run = tokio::spawn(worker.run());
    let done = wait_status(&mut client, &t.id, 4, Duration::from_secs(10)).await;
    assert_eq!(done.output, r#"{"n":1}"#);
    assert_eq!(
        server.node.engine.checkpoints_for_task(&t.id).unwrap()[0].step,
        "one"
    );
    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

#[tokio::test]
async fn e2e_clients_and_workers_ride_through_a_dead_node_and_a_restart() {
    let backing: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let store = Store::wrap(backing.clone(), "", true, "shared-memory");
    let port = free_port().await;
    let server = start_server(store.clone(), "retry", port).await;
    // Nothing listens on the first address: the client and the worker skip it.
    let dead = format!("http://127.0.0.1:{}", free_port().await);
    let addrs = [dead.clone(), server.addr.clone()];
    let mut client = ValkaClient::builder()
        .addrs(&addrs)
        .connect()
        .await
        .unwrap();
    let a = client.create_task("q", "t", None).await.unwrap();
    assert!(
        a.idempotency_key.starts_with("auto-"),
        "every create carries a key, so a retry cannot make a second task"
    );
    // An explicit key twice: one task.
    let keyed = valka_proto::CreateTaskRequest {
        queue_name: "q".into(),
        task_name: "t".into(),
        idempotency_key: "order-7".into(),
        ..Default::default()
    };
    let k1 = client.create_task_request(keyed.clone()).await.unwrap();
    let k2 = client.create_task_request(keyed).await.unwrap();
    assert_eq!(k1.id, k2.id);
    server.node.engine.sync().await.unwrap();

    // The node crashes and comes back a second later on the same bucket and port. A create
    // issued meanwhile is retried until it lands, once.
    let _ = server.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server.handle).await;
    drop(server.node);
    let restart = {
        let store = store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            start_server(store, "retry", port).await
        })
    };
    let b = client.create_task("q", "t", None).await.unwrap();
    let server = restart.await.unwrap();
    let all = server.node.engine.list_tasks(None, None, 10, 0);
    assert_eq!(all.len(), 3, "a, the keyed task and b: {all:?}");
    assert!(all.iter().any(|t| t.id == b.id));

    // A worker given both addresses runs them all.
    let worker = ValkaWorker::builder()
        .server_addrs(&addrs)
        .queues(&["q"])
        .concurrency(4)
        .handler(|_ctx| async move { Ok(serde_json::json!({})) })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let run = tokio::spawn(worker.run());
    for id in [&a.id, &k1.id, &b.id] {
        wait_status(&mut client, id, 4, Duration::from_secs(15)).await;
    }
    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

/// Delays every result by `delay` before passing it on, like an owner across the network
/// or a slow bucket.
struct SlowResults {
    inner: Arc<dyn valka_dispatcher::TaskRouter>,
    delay: Duration,
}

#[async_trait::async_trait]
impl valka_dispatcher::TaskRouter for SlowResults {
    async fn dispatch(
        &self,
        task_id: &str,
        worker_id: &str,
    ) -> Result<valka_engine::DispatchInfo, valka_core::ServerError> {
        self.inner.dispatch(task_id, worker_id).await
    }
    async fn return_run(&self, task_id: &str, run_id: &str) {
        self.inner.return_run(task_id, run_id).await
    }
    async fn unoffer(&self, task_id: &str) {
        self.inner.unoffer(task_id).await
    }
    async fn report_result(
        &self,
        task_id: &str,
        run_id: &str,
        result: valka_engine::RunResult,
    ) -> valka_engine::ResultOutcome {
        tokio::time::sleep(self.delay).await;
        self.inner.report_result(task_id, run_id, result).await
    }
    async fn heartbeat(&self, worker_id: &str, task_ids: &[String]) {
        self.inner.heartbeat(worker_id, task_ids).await
    }
    async fn checkpoint(
        &self,
        task_id: &str,
        run_id: &str,
        step: &str,
        output: serde_json::Value,
    ) -> Result<valka_engine::CheckpointView, valka_core::ServerError> {
        self.inner.checkpoint(task_id, run_id, step, output).await
    }
    async fn signal_delivered(&self, signal_id: &str) {
        self.inner.signal_delivered(signal_id).await
    }
    async fn signal_acked(&self, signal_id: &str) {
        self.inner.signal_acked(signal_id).await
    }
    async fn reset_signals(&self, task_id: &str) {
        self.inner.reset_signals(task_id).await
    }
    async fn reattach(
        &self,
        task_id: &str,
        run_id: &str,
        attempt: i32,
        worker_id: &str,
    ) -> Result<valka_engine::ReattachOutcome, valka_core::ServerError> {
        self.inner
            .reattach(task_id, run_id, attempt, worker_id)
            .await
    }
}

#[tokio::test]
async fn e2e_one_worker_finishes_results_concurrently() {
    // Found by the benchmark: the server applied a worker's results one after another, each
    // waiting for its group commit, so one worker finished about 20 tasks a second however
    // many slots it had.
    let port = free_port().await;
    let server = start_server(Store::memory(), "e2e", port).await;
    let dispatcher = server.node.dispatcher.clone();
    dispatcher.set_router(Arc::new(SlowResults {
        inner: dispatcher.router(),
        delay: Duration::from_millis(500),
    }));

    let worker = ValkaWorker::builder()
        .name("wide-worker")
        .server_addr(&server.addr)
        .queues(&["wide"])
        .concurrency(32)
        .handler(|_ctx| async move { Ok(serde_json::json!({})) })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let run = tokio::spawn(worker.run());

    let mut client = ValkaClient::connect(&server.addr).await.unwrap();
    let mut ids = Vec::new();
    for _ in 0..32 {
        ids.push(client.create_task("wide", "t", None).await.unwrap().id);
    }
    // 32 results at 0.5 s each: 16 s one after another, about 0.5 s together.
    let started = std::time::Instant::now();
    for id in &ids {
        wait_status(&mut client, id, 4, Duration::from_secs(20)).await;
    }
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "32 results took {:?}: they were applied one at a time",
        started.elapsed()
    );
    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}
