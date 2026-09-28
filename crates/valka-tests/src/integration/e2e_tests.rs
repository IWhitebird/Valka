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
    let node = TestNode::on_store(store, node_id).await;
    let (shutdown, rx) = watch::channel(false);
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let (engine, dispatcher, event_tx, nid, logs) = (
        node.engine.clone(),
        node.dispatcher.clone(),
        node.event_tx.clone(),
        node.node_id.clone(),
        node.logs.clone(),
    );
    let handle = tokio::spawn(async move {
        valka_server::grpc::serve_grpc(addr, engine, dispatcher, event_tx, nid, logs, rx)
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
    let recs = valka_wal::reader::read_all(&server.node.store, "e2e", None)
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
