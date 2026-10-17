//! Workers attached to one node running tasks owned by other nodes (PHASE2 §10): remote
//! fetch, dispatch recorded by the owner, results, heartbeats, checkpoints, cancels and
//! signals crossing nodes, and the cluster-wide event stream. Real localhost gRPC.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use valka_proto::{TaskResult, WorkerResponse, worker_response};
use valka_sdk::ValkaWorker;

use super::cluster_routing_tests::*;
use super::helpers::*;

/// Start node `i`'s public gRPC API (for SDK workers). Returns its address.
async fn serve_public(c: &Cluster, i: usize) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let n = &c.nodes[i];
    tokio::spawn(valka_server::grpc::serve_grpc(
        addr,
        n.api.clone(),
        n.cluster_events.clone(),
        n.logs.clone(),
        c.shutdown.subscribe(),
    ));
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    format!("http://{addr}")
}

async fn create_via(node: &TestNode, queue: &str, input: serde_json::Value) -> String {
    let (s, b) = call(
        node,
        post(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": queue, "task_name": "t", "input": input}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    b["id"].as_str().unwrap().to_string()
}

async fn status_via(node: &TestNode, id: &str) -> serde_json::Value {
    let (s, b) = call(node, get(&format!("/api/v1/tasks/{id}"))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    b
}

async fn wait_status(node: &TestNode, id: &str, want: &str, secs: u64) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let t = status_via(node, id).await;
        if t["status"] == want {
            return t;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "task {id} stuck in {} (wanted {want})",
            t["status"]
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A fake worker on node `i`: registered with the dispatcher and matching like a real
/// stream, its server-to-worker messages on a channel.
async fn fake_worker(
    c: &Cluster,
    i: usize,
    queue: &str,
) -> (
    valka_core::WorkerId,
    tokio::sync::mpsc::Receiver<WorkerResponse>,
) {
    let n = &c.nodes[i];
    let (id, rx) = n.register_worker(&[queue], 1).await;
    let (d, wid, q) = (n.dispatcher.clone(), id.clone(), vec![queue.to_string()]);
    tokio::spawn(async move { d.run_worker_match_loop(wid, q).await });
    (id, rx)
}

async fn next_message(
    rx: &mut tokio::sync::mpsc::Receiver<WorkerResponse>,
    secs: u64,
    pick: impl Fn(&worker_response::Response) -> bool,
) -> worker_response::Response {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(left, rx.recv())
            .await
            .expect("timed out waiting for a worker message")
            .expect("worker channel closed");
        if let Some(r) = msg.response
            && pick(&r)
        {
            return r;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_on_one_node_runs_tasks_owned_by_every_node() {
    let c = cluster(3).await;
    let addr = serve_public(&c, 1).await;
    let handled = Arc::new(AtomicUsize::new(0));
    let h = handled.clone();
    let worker = ValkaWorker::builder()
        .name("remote-worker")
        .server_addr(&addr)
        .queues(&["jobs"])
        .concurrency(4)
        .handler(move |ctx| {
            let h = h.clone();
            async move {
                let input: serde_json::Value = ctx.input().unwrap();
                // A step: the checkpoint goes to the task's owner.
                let doubled: i64 = ctx
                    .step("double", || async move {
                        Ok::<_, String>(input["n"].as_i64().unwrap() * 2)
                    })
                    .await?;
                ctx.log("done").await;
                h.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "doubled": doubled }))
            }
        })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let run = tokio::spawn(worker.run());

    // Created through every node: placement prefers local partitions, so the tasks live
    // on all three nodes. Only node 1 has a worker.
    let mut ids = Vec::new();
    for n in 0..24 {
        ids.push(create_via(&c.nodes[n % 3], "jobs", serde_json::json!({ "n": n })).await);
    }
    let owners: HashSet<usize> = ids.iter().filter_map(|id| c.owner_of(id)).collect();
    assert_eq!(owners.len(), 3, "tasks on every node");

    for (n, id) in ids.iter().enumerate() {
        let t = wait_status(&c.nodes[(n + 2) % 3], id, "COMPLETED", 20).await;
        assert_eq!(t["output"]["doubled"], (n as i64) * 2);
        assert_eq!(t["attempt_count"], 1, "no task ran twice");
        let (_, runs) = call(&c.nodes[n % 3], get(&format!("/api/v1/tasks/{id}/runs"))).await;
        assert_eq!(
            runs[0]["assigned_node_id"], "n1",
            "the run lives on the worker's node"
        );
        let (_, cps) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{id}/checkpoints"))).await;
        assert_eq!(cps[0]["step"], "double", "checkpoint recorded by the owner");
    }
    assert_eq!(handled.load(Ordering::SeqCst), 24);
    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancels_signals_heartbeats_and_results_cross_nodes() {
    let c = cluster(3).await;
    // A task owned by node 0, run by a worker on node 1.
    let (worker, mut rx) = fake_worker(&c, 1, "ops").await;
    let id = loop {
        let id = create_via(&c.nodes[0], "ops", serde_json::json!({})).await;
        if c.owner_of(&id) == Some(0) {
            break id;
        }
    };
    let assignment = match next_message(
        &mut rx,
        10,
        |r| matches!(r, worker_response::Response::TaskAssignment(a) if a.task_id == id),
    )
    .await
    {
        worker_response::Response::TaskAssignment(a) => a,
        _ => unreachable!(),
    };
    let t = wait_status(&c.nodes[2], &id, "RUNNING", 5).await;
    assert_eq!(t["attempt_count"], 1);

    // Signal sent through node 0 (the owner) reaches the worker on node 1.
    let (s, body) = call(
        &c.nodes[0],
        post(
            &format!("/api/v1/tasks/{id}/signal"),
            serde_json::json!({"signal_name": "ping", "payload": {"x": 1}}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(body["delivered"], true, "{body}");
    let sig = match next_message(&mut rx, 5, |r| {
        matches!(r, worker_response::Response::TaskSignal(_))
    })
    .await
    {
        worker_response::Response::TaskSignal(s) => s,
        _ => unreachable!(),
    };
    assert_eq!(sig.signal_name, "ping");
    // The worker acks through its node; the owner records it.
    c.nodes[1]
        .dispatcher
        .handle_signal_ack(&valka_proto::SignalAck {
            signal_id: sig.signal_id.clone(),
        })
        .await;
    for _ in 0..50 {
        let (_, sigs) = call(&c.nodes[2], get(&format!("/api/v1/tasks/{id}/signals"))).await;
        if sigs[0]["status"] == "ACKNOWLEDGED" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, sigs) = call(&c.nodes[2], get(&format!("/api/v1/tasks/{id}/signals"))).await;
    assert_eq!(sigs[0]["status"], "ACKNOWLEDGED");

    // A heartbeat through node 1 extends the lease on node 0.
    let (_, before) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{id}/runs"))).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    c.nodes[1]
        .dispatcher
        .handle_heartbeat(
            &worker,
            valka_proto::Heartbeat {
                active_task_ids: vec![id.clone()],
                timestamp_ms: 0,
            },
        )
        .await;
    let mut moved = false;
    for _ in 0..50 {
        let (_, after) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{id}/runs"))).await;
        if after[0]["last_heartbeat"] != before[0]["last_heartbeat"] {
            moved = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(moved, "the owner saw the heartbeat");

    // The result goes from node 1 to the owner and is acked end to end.
    c.nodes[1]
        .dispatcher
        .handle_task_result(
            &worker,
            TaskResult {
                task_id: id.clone(),
                task_run_id: assignment.task_run_id.clone(),
                success: true,
                retryable: false,
                output: "{\"ok\":true}".into(),
                error_message: String::new(),
            },
        )
        .await;
    let ack = match next_message(&mut rx, 5, |r| {
        matches!(r, worker_response::Response::ResultAck(_))
    })
    .await
    {
        worker_response::Response::ResultAck(a) => a,
        _ => unreachable!(),
    };
    assert_eq!(ack.status, valka_proto::ResultStatus::Applied as i32);
    wait_status(&c.nodes[2], &id, "COMPLETED", 5).await;

    // A cancel issued through node 2 reaches the worker on node 1.
    let id2 = loop {
        let id = create_via(&c.nodes[0], "ops", serde_json::json!({})).await;
        if c.owner_of(&id) == Some(0) {
            break id;
        }
    };
    next_message(
        &mut rx,
        10,
        |r| matches!(r, worker_response::Response::TaskAssignment(a) if a.task_id == id2),
    )
    .await;
    wait_status(&c.nodes[0], &id2, "RUNNING", 5).await;
    let (s, _) = call(
        &c.nodes[2],
        post(
            &format!("/api/v1/tasks/{id2}/cancel"),
            serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    match next_message(&mut rx, 5, |r| {
        matches!(r, worker_response::Response::TaskCancellation(_))
    })
    .await
    {
        worker_response::Response::TaskCancellation(cn) => assert_eq!(cn.task_id, id2),
        _ => unreachable!(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribers_see_events_from_every_node() {
    let c = cluster(3).await;
    let mut rx = c.nodes[2].cluster_events.subscribe();
    // The relay connects to peers within a tick once someone listens.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let mut ids = HashSet::new();
    for i in 0..9 {
        ids.insert(create_via(&c.nodes[i % 3], "ev", serde_json::json!({})).await);
    }
    let owners: HashSet<usize> = ids.iter().filter_map(|id| c.owner_of(id)).collect();
    assert!(owners.len() >= 2);
    let mut seen = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while seen.len() < ids.len() {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let ev = tokio::time::timeout(left, rx.recv())
            .await
            .expect("missing events")
            .unwrap();
        if ids.contains(&ev.task_id) {
            seen.insert(ev.task_id);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_survives_the_owners_death() {
    let c = cluster(3).await;
    let (worker, mut rx) = fake_worker(&c, 1, "fo").await;
    let id = loop {
        let id = create_via(&c.nodes[0], "fo", serde_json::json!({})).await;
        if c.owner_of(&id) == Some(0) {
            break id;
        }
    };
    let assignment = match next_message(
        &mut rx,
        10,
        |r| matches!(r, worker_response::Response::TaskAssignment(a) if a.task_id == id),
    )
    .await
    {
        worker_response::Response::TaskAssignment(a) => a,
        _ => unreachable!(),
    };
    wait_status(&c.nodes[0], &id, "RUNNING", 5).await;
    c.nodes[0].engine.sync().await.unwrap();
    // The owner dies; the worker on node 1 keeps running and reports after the takeover.
    c.nodes[0].engine.halt();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (s, b) = call(&c.nodes[2], get(&format!("/api/v1/tasks/{id}"))).await;
        // The halted node still believes it owns the shard: ask the survivors.
        let shard = c.nodes[1].engine.shard_of(&id).unwrap();
        let taken = c.nodes[1..].iter().any(|n| n.engine.owns(shard));
        if s == StatusCode::OK && taken {
            assert_eq!(b["status"], "RUNNING", "the run survived the takeover");
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "no takeover");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    c.nodes[1]
        .dispatcher
        .handle_task_result(
            &worker,
            TaskResult {
                task_id: id.clone(),
                task_run_id: assignment.task_run_id,
                success: true,
                retryable: false,
                output: "1".into(),
                error_message: String::new(),
            },
        )
        .await;
    let ack = match next_message(&mut rx, 5, |r| {
        matches!(r, worker_response::Response::ResultAck(_))
    })
    .await
    {
        worker_response::Response::ResultAck(a) => a,
        _ => unreachable!(),
    };
    assert_eq!(ack.status, valka_proto::ResultStatus::Applied as i32);
    let t = wait_status(&c.nodes[2], &id, "COMPLETED", 5).await;
    assert_eq!(t["attempt_count"], 1, "the task ran once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tasks_left_in_partitions_removed_by_a_shrink_reach_remote_workers() {
    let c = cluster(3).await;
    // 24 tasks over the default 16 partitions, on every node; no worker yet.
    let mut ids = Vec::new();
    for n in 0..24 {
        ids.push(create_via(&c.nodes[n % 3], "shrink", serde_json::json!({ "n": n })).await);
    }
    let owners: HashSet<usize> = ids.iter().filter_map(|id| c.owner_of(id)).collect();
    assert_eq!(owners.len(), 3, "tasks on every node");

    // Shrink to one partition: the other 15 take no new tasks but still hold these.
    valka_wal::queues::set_partitions(c.nodes[0].engine.store(), "shrink", 1, 16)
        .await
        .unwrap();
    for n in &c.nodes {
        n.engine.invalidate_queue("shrink");
    }

    // Only node 1 has a worker: it must fetch from the owners of the removed partitions.
    let addr = serve_public(&c, 1).await;
    let worker = ValkaWorker::builder()
        .name("after-shrink")
        .server_addr(&addr)
        .queues(&["shrink"])
        .concurrency(4)
        .handler(|_ctx| async move { Ok(serde_json::json!({})) })
        .build()
        .await
        .unwrap();
    let stop = worker.shutdown_handle();
    let run = tokio::spawn(worker.run());
    for id in &ids {
        wait_status(&c.nodes[0], id, "COMPLETED", 20).await;
    }
    stop.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}
