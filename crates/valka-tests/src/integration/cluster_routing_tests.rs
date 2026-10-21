//! Several nodes on one bucket, talking over real internal gRPC on localhost: any node
//! accepts any request and routes it to the shard's owner (docs/wal/PHASE2.md §11).
//! Real time: leases and placement run with short intervals.

use std::collections::HashMap;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use valka_engine::EngineConfig;
use valka_wal::Store;

use super::helpers::*;

pub(super) const SHARDS: u32 = 64;
pub(super) const TOKEN: &str = "test-cluster-token";

pub(super) struct Cluster {
    pub(super) nodes: Vec<TestNode>,
    pub(super) shutdown: tokio::sync::watch::Sender<bool>,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for n in &self.nodes {
            n.engine.halt();
        }
    }
}

fn cfg(node: &str, internal_addr: &str) -> EngineConfig {
    let mut c = EngineConfig::for_tests(node);
    c.cluster.num_shards = SHARDS;
    c.cluster.lease_interval_ms = 200;
    c.cluster.lease_timeout_ms = 1_500;
    c.cluster.lease_safety_margin_ms = 500;
    c.cluster.membership_interval_ms = 100;
    c.cluster.placement_interval_ms = 100;
    c.advert.internal_addr = Some(internal_addr.to_string());
    c
}

pub(super) struct Tls {
    pub(super) server: tonic::transport::ServerTlsConfig,
    pub(super) client: tonic::transport::ClientTlsConfig,
}

pub(super) async fn start_node(
    backing: &std::sync::Arc<dyn object_store::ObjectStore>,
    name: &str,
    server_token: Option<&str>,
    client_token: Option<&str>,
    tls: Option<Tls>,
    shutdown: &tokio::sync::watch::Sender<bool>,
) -> TestNode {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let store = Store::wrap(backing.clone(), "", true, "memory");
    let (server_tls, client_tls) = match tls {
        Some(t) => (Some(t.server), Some(t.client)),
        None => (None, None),
    };
    let node = TestNode::build(
        store,
        cfg(name, &addr),
        client_token.map(str::to_string),
        client_tls,
    )
    .await;
    tokio::spawn(valka_server::internal_grpc::serve_internal(
        listener,
        node.api.clone(),
        node.event_tx.clone(),
        server_token.map(str::to_string),
        server_tls,
        shutdown.subscribe(),
    ));
    node
}

pub(super) async fn cluster(n: usize) -> Cluster {
    let backing: std::sync::Arc<dyn object_store::ObjectStore> =
        std::sync::Arc::new(object_store::memory::InMemory::new());
    let (shutdown, _) = tokio::sync::watch::channel(false);
    let mut nodes = Vec::new();
    for i in 0..n {
        nodes.push(
            start_node(
                &backing,
                &format!("n{i}"),
                Some(TOKEN),
                Some(TOKEN),
                None,
                &shutdown,
            )
            .await,
        );
    }
    let c = Cluster { nodes, shutdown };
    c.converge().await;
    c
}

impl Cluster {
    /// Every node serves its share and every shard is served.
    pub(super) async fn converge(&self) {
        for _ in 0..200 {
            let total: usize = self
                .nodes
                .iter()
                .map(|n| n.engine.owned_shards().len())
                .sum();
            let all_busy = self
                .nodes
                .iter()
                .all(|n| !n.engine.owned_shards().is_empty());
            let all_see_all = self
                .nodes
                .iter()
                .all(|n| n.engine.live_peers().len() == self.nodes.len() - 1);
            if total == SHARDS as usize && all_busy && all_see_all {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("cluster did not converge");
    }

    pub(super) fn owner_of(&self, task_id: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|n| n.engine.shard_of(task_id).is_some_and(|s| n.engine.owns(s)))
    }
}

pub(super) async fn call(node: &TestNode, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let resp = node.router().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if body.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

pub(super) fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

pub(super) fn get(uri: &str) -> Request<Body> {
    Request::builder().uri(uri).body(Body::empty()).unwrap()
}

pub(super) fn delete(uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rest_works_through_any_node_of_three() {
    let c = cluster(3).await;
    let mut ids: Vec<String> = Vec::new();
    for i in 0..30 {
        let via = &c.nodes[i % 3];
        let (status, body) = call(
            via,
            post(
                "/api/v1/tasks",
                serde_json::json!({"queue_name": format!("q{}", i % 4), "task_name": "t"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        ids.push(body["id"].as_str().unwrap().to_string());
    }
    // The tasks really are spread over the nodes.
    let mut per_owner: HashMap<usize, usize> = HashMap::new();
    for id in &ids {
        *per_owner.entry(c.owner_of(id).expect("owned")).or_default() += 1;
    }
    assert!(
        per_owner.len() >= 2,
        "tasks on one node only: {per_owner:?}"
    );

    // Reads through every node, whoever owns the task.
    for id in &ids {
        for node in &c.nodes {
            let (status, body) = call(node, get(&format!("/api/v1/tasks/{id}"))).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["id"], *id);
            assert_eq!(body["status"], "PENDING");
        }
    }
    // Cluster-wide list and counts from any node.
    for node in &c.nodes {
        let resp = node
            .router()
            .oneshot(get("/api/v1/tasks?limit=100"))
            .await
            .unwrap();
        assert_eq!(resp.headers()["x-valka-partial"], "false");
        let body = parse_response_json(resp).await;
        assert_eq!(body.as_array().unwrap().len(), 30);
        let (_, page) = call(node, get("/api/v1/tasks?limit=10&offset=25")).await;
        assert_eq!(page.as_array().unwrap().len(), 5);
        let (_, q1) = call(node, get("/api/v1/tasks?queue_name=q1&limit=100")).await;
        assert!(
            q1.as_array()
                .unwrap()
                .iter()
                .all(|t| t["queue_name"] == "q1")
        );
    }

    // Commands routed to remote owners.
    let remote = |via: usize| {
        ids.iter()
            .find(|id| c.owner_of(id) != Some(via))
            .unwrap()
            .clone()
    };
    let id = remote(0);
    let (status, body) = call(
        &c.nodes[0],
        post(&format!("/api/v1/tasks/{id}/cancel"), serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "CANCELLED");
    let (status, _) = call(
        &c.nodes[1],
        post(&format!("/api/v1/tasks/{id}/cancel"), serde_json::json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the owner's error comes back as is"
    );

    let id = remote(1);
    let (status, body) = call(
        &c.nodes[1],
        post(
            &format!("/api/v1/tasks/{id}/signal"),
            serde_json::json!({"signal_name": "ping", "payload": {"a": 1}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, sigs) = call(&c.nodes[2], get(&format!("/api/v1/tasks/{id}/signals"))).await;
    assert_eq!(sigs.as_array().unwrap().len(), 1);
    let (status, runs) = call(&c.nodes[2], get(&format!("/api/v1/tasks/{id}/runs"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(runs, serde_json::json!([]));
    let (status, _) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{id}/checkpoints"))).await;
    assert_eq!(status, StatusCode::OK);

    let id = remote(2);
    let (status, _) = call(&c.nodes[2], delete(&format!("/api/v1/tasks/{id}"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{id}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unknown ids are 404 everywhere, without asking anyone.
    let (status, _) = call(&c.nodes[1], get("/api/v1/tasks/not-a-uuid")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Cluster overview from any node sees all three nodes and every shard owned.
    let (_, overview) = call(&c.nodes[2], get("/api/v1/cluster")).await;
    assert_eq!(overview["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(overview["health"]["unowned_shards"], 0, "{overview}");
    assert_eq!(overview["health"]["status"], "ok", "{overview}");
    let (_, shards) = call(&c.nodes[0], get("/api/v1/cluster/shards")).await;
    let shards = shards.as_array().unwrap();
    assert_eq!(shards.len(), SHARDS as usize);
    assert!(shards.iter().all(|s| s["owner"].is_string()));

    // Clear through one node clears every node.
    let (status, body) = call(&c.nodes[1], delete("/api/v1/tasks")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["deleted_count"], 29);
    for node in &c.nodes {
        assert_eq!(node.engine.count_tasks(None, None), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cluster_views_come_from_every_node() {
    let c = cluster(3).await;
    let (status, body) = call(
        &c.nodes[0],
        post(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "views", "task_name": "t"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let owner = c.owner_of(&id).expect("owned");
    let shard = c.nodes[owner].engine.shard_of(&id).unwrap();
    let owner_id = c.nodes[owner].engine.node_id().to_string();

    for node in &c.nodes {
        // A shard's detail comes from its owner, through any node.
        let (status, d) = call(node, get(&format!("/api/v1/cluster/shards/{}", shard.0))).await;
        assert_eq!(status, StatusCode::OK, "{d}");
        assert_eq!(d["owner"], owner_id.as_str());
        assert_eq!(d["phase"], "serving");
        assert!(d["gen"].as_u64().unwrap() >= 1);
        assert_eq!(d["queues"]["views"]["pending"], 1);

        // The shard map covers every shard, each with its owner.
        let (_, rows) = call(node, get("/api/v1/cluster/shards")).await;
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), SHARDS as usize);
        assert!(
            rows.iter().all(|r| r["owner"].is_string()),
            "every shard owned"
        );

        // The overview lists every node with its incarnation and shard count.
        let (_, o) = call(node, get("/api/v1/cluster")).await;
        assert_eq!(o["num_shards"], SHARDS);
        assert_eq!(o["health"]["unowned_shards"], 0);
        let nodes = o["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let owned: u64 = nodes
            .iter()
            .map(|n| n["shards_owned"].as_u64().unwrap())
            .sum();
        assert_eq!(owned, SHARDS as u64);
        assert!(nodes.iter().all(|n| n["inc"].as_u64() == Some(1)));

        // Membership: this node's view of every lease.
        let (_, m) = call(node, get("/api/v1/cluster/members")).await;
        let m = m.as_array().unwrap();
        assert_eq!(m.len(), 3, "{m:?}");
        assert!(
            m.iter()
                .all(|r| r["status"] == "alive" && r["eligible"] == true)
        );
        assert!(
            m.iter()
                .all(|r| r["unsplit"].as_array().unwrap().is_empty()),
            "a live incarnation is not an unsplit log: {m:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idempotency_key_dedupes_across_entry_nodes() {
    let c = cluster(3).await;
    let req = serde_json::json!({"queue_name": "orders", "task_name": "t", "idempotency_key": "order-42"});
    let (status, first) = call(&c.nodes[0], post("/api/v1/tasks", req.clone())).await;
    assert_eq!(status, StatusCode::CREATED);
    for node in &c.nodes[1..] {
        let (status, body) = call(node, post("/api/v1/tasks", req.clone())).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["id"], first["id"], "the key's task, through any node");
    }
    let (_, all) = call(&c.nodes[2], get("/api/v1/tasks?queue_name=orders")).await;
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert_eq!(all[0]["id"], first["id"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_without_the_cluster_token_are_refused() {
    let backing: std::sync::Arc<dyn object_store::ObjectStore> =
        std::sync::Arc::new(object_store::memory::InMemory::new());
    let (shutdown, _) = tokio::sync::watch::channel(false);
    let good = start_node(&backing, "good", Some(TOKEN), Some(TOKEN), None, &shutdown).await;
    let rogue = start_node(
        &backing,
        "rogue",
        Some(TOKEN),
        Some("wrong"),
        None,
        &shutdown,
    )
    .await;
    let c = Cluster {
        nodes: vec![good, rogue],
        shutdown,
    };
    c.converge().await;
    // A task owned by `good`, asked for through `rogue`: the forward is unauthenticated.
    let id = loop {
        let (s, b) = call(
            &c.nodes[0],
            post(
                "/api/v1/tasks",
                serde_json::json!({"queue_name": "q", "task_name": "t"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        let id = b["id"].as_str().unwrap().to_string();
        if c.owner_of(&id) == Some(0) {
            break id;
        }
    };
    let (status, body) = call(&c.nodes[1], get(&format!("/api/v1/tasks/{id}"))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    // The node with the right token is served.
    let peer = c.nodes[1]
        .engine
        .live_peers()
        .into_iter()
        .find(|(n, _)| n == "good")
        .unwrap()
        .1
        .internal_addr
        .unwrap();
    let ok =
        valka_cluster::PeerClients::new("probe", Some(TOKEN.into()), None, Duration::from_secs(2));
    assert_eq!(ok.ping(&peer).await.unwrap().node_id, "good");
    let bad = valka_cluster::PeerClients::new("probe", None, None, Duration::from_secs(2));
    assert!(bad.ping(&peer).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_reach_the_new_owner_after_a_takeover() {
    let c = cluster(3).await;
    let mut ids = Vec::new();
    for _ in 0..20 {
        let (s, b) = call(
            &c.nodes[0],
            post(
                "/api/v1/tasks",
                serde_json::json!({"queue_name": "q", "task_name": "t"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        ids.push(b["id"].as_str().unwrap().to_string());
    }
    let victim = c.owner_of(&ids[0]).unwrap();
    let via = (victim + 1) % 3;
    c.nodes[victim].engine.halt();
    // Until the takeover finishes the shard is unavailable; afterwards it is served.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (status, body) = call(&c.nodes[via], get(&format!("/api/v1/tasks/{}", ids[0]))).await;
        if status == StatusCode::OK {
            assert_eq!(body["status"], "PENDING");
            break;
        }
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(tokio::time::Instant::now() < deadline, "never taken over");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Every acked task is served again. The first read above may have been answered by
    // the halted node before its lease aged out, so the others' shards may still be in
    // the middle of their takeover: wait for each the same way.
    for id in &ids {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let (status, body) = call(&c.nodes[via], get(&format!("/api/v1/tasks/{id}"))).await;
            if status == StatusCode::OK {
                break;
            }
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
            assert!(
                tokio::time::Instant::now() < deadline,
                "acked task {id} never served after the takeover"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    // Membership names the dead node and the guardian that tombstoned it; no shard is
    // left unowned. (The reads above may have been served before the takeover: a halted
    // node answers until its lease is too old.)
    let victim_id = c.nodes[victim].engine.node_id().to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let row = loop {
        let (_, m) = call(&c.nodes[via], get("/api/v1/cluster/members")).await;
        let row = m
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["node_id"] == victim_id.as_str())
            .unwrap()
            .clone();
        if row["status"] == "dead" {
            break row;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never tombstoned: {row}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let guardian = row["dead_by"].as_str().expect("dead_by");
    assert!(guardian != victim_id && guardian.starts_with('n'), "{row}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let (_, unowned) = call(&c.nodes[via], get("/api/v1/cluster/shards?unowned=true")).await;
        let unowned = unowned.as_array().unwrap();
        if unowned.is_empty() {
            break;
        }
        assert!(unowned.iter().all(|r| r["owner"].is_null()));
        assert!(
            tokio::time::Instant::now() < deadline,
            "shards left unowned"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn certs(names: &[&str]) -> (String, Vec<(String, String)>) {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let leaves = names
        .iter()
        .map(|_| {
            let key = KeyPair::generate().unwrap();
            let params =
                CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
                    .unwrap();
            let cert = params.signed_by(&key, &ca, &ca_key).unwrap();
            (cert.pem(), key.serialize_pem())
        })
        .collect();
    (ca.pem(), leaves)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutual_tls_between_nodes() {
    use tonic::transport::{Certificate, ClientTlsConfig, Identity, ServerTlsConfig};
    let (ca, leaves) = certs(&["a", "b"]);
    let tls = |i: usize| {
        let (cert, key) = &leaves[i];
        Tls {
            server: ServerTlsConfig::new()
                .identity(Identity::from_pem(cert, key))
                .client_ca_root(Certificate::from_pem(&ca)),
            client: ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(&ca))
                .identity(Identity::from_pem(cert, key)),
        }
    };
    let backing: std::sync::Arc<dyn object_store::ObjectStore> =
        std::sync::Arc::new(object_store::memory::InMemory::new());
    let (shutdown, _) = tokio::sync::watch::channel(false);
    let a = start_node(
        &backing,
        "a",
        Some(TOKEN),
        Some(TOKEN),
        Some(tls(0)),
        &shutdown,
    )
    .await;
    let b = start_node(
        &backing,
        "b",
        Some(TOKEN),
        Some(TOKEN),
        Some(tls(1)),
        &shutdown,
    )
    .await;
    let c = Cluster {
        nodes: vec![a, b],
        shutdown,
    };
    c.converge().await;
    let mut seen_remote = false;
    for _ in 0..20 {
        let (s, body) = call(
            &c.nodes[0],
            post(
                "/api/v1/tasks",
                serde_json::json!({"queue_name": "q", "task_name": "t"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let id = body["id"].as_str().unwrap().to_string();
        let (s, _) = call(&c.nodes[1], get(&format!("/api/v1/tasks/{id}"))).await;
        assert_eq!(s, StatusCode::OK);
        seen_remote |= c.owner_of(&id) == Some(0);
    }
    assert!(seen_remote, "some reads crossed nodes over mTLS");
    // A client without a certificate cannot even connect.
    let addr = c.nodes[1].engine.live_peers()[0]
        .1
        .internal_addr
        .clone()
        .unwrap();
    let no_cert = valka_cluster::PeerClients::new(
        "probe",
        Some(TOKEN.into()),
        Some(ClientTlsConfig::new().ca_certificate(Certificate::from_pem(&ca))),
        Duration::from_secs(2),
    );
    assert!(no_cert.ping(&addr).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idempotency_holds_across_a_partition_resize() {
    use valka_core::{jump_hash, partition_shard};
    let c = cluster(2).await;
    // A key whose partition lands on another shard once the queue has 32 partitions.
    let key = (0..)
        .map(|i| format!("k{i}"))
        .find(|k| {
            partition_shard("orders", jump_hash(k, 16), SHARDS)
                != partition_shard("orders", jump_hash(k, 32), SHARDS)
        })
        .unwrap();
    let req = serde_json::json!({"queue_name": "orders", "task_name": "t", "idempotency_key": key});
    let (status, first) = call(&c.nodes[0], post("/api/v1/tasks", req.clone())).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    valka_wal::queues::set_partitions(c.nodes[0].engine.store(), "orders", 32, 16)
        .await
        .unwrap();
    for n in &c.nodes {
        n.engine.invalidate_queue("orders");
    }
    for n in &c.nodes {
        let (status, body) = call(n, post("/api/v1/tasks", req.clone())).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the old partition still dedupes: {body}"
        );
        assert_eq!(body["id"], first["id"]);
    }
    let fresh =
        serde_json::json!({"queue_name": "orders", "task_name": "t", "idempotency_key": "new-key"});
    let (status, _) = call(&c.nodes[1], post("/api/v1/tasks", fresh)).await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_api_routes_like_rest() {
    let c = cluster(2).await;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let (api, events, logs, rx) = (
        c.nodes[1].api.clone(),
        c.nodes[1].cluster_events.clone(),
        c.nodes[1].logs.clone(),
        c.shutdown.subscribe(),
    );
    tokio::spawn(valka_server::grpc::serve_grpc(
        addr,
        api,
        events,
        logs,
        valka_server::auth::ApiKeys::open(),
        rx,
    ));
    let mut client = None;
    for _ in 0..50 {
        if let Ok(cl) = valka_sdk::ValkaClient::connect(&format!("http://{addr}")).await {
            client = Some(cl);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut client = client.expect("grpc server up");
    // Tasks created through node 0 live on node 0 (placement prefers local partitions):
    // reading them through node 1's gRPC API crosses nodes.
    let mut remote = 0;
    for _ in 0..20 {
        let (s, b) = call(
            &c.nodes[0],
            post(
                "/api/v1/tasks",
                serde_json::json!({"queue_name": "q", "task_name": "t"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        let id = b["id"].as_str().unwrap().to_string();
        let back = client.get_task(&id).await.unwrap();
        assert_eq!(back.id, id);
        if c.owner_of(&id) == Some(0) {
            remote += 1;
        }
    }
    assert!(remote > 0, "some gRPC reads were forwarded");
    let t = client.create_task("q", "t", None).await.unwrap();
    let (status, _) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{}", t.id))).await;
    assert_eq!(status, StatusCode::OK);
}

const ADMIN: &str = "admin-secret";

async fn admin_call(
    node: &TestNode,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = node
        .router_with_admin(Some(ADMIN))
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_api_needs_its_token() {
    let c = cluster(2).await;
    // Disabled when no token is configured.
    let resp = c.nodes[0]
        .router()
        .oneshot(post(
            "/api/v1/cluster/shards/0/move",
            serde_json::json!({"to": "n1"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let (s, _) = admin_call(
        &c.nodes[0],
        "POST",
        "/api/v1/cluster/shards/0/move",
        None,
        Some(serde_json::json!({"to": "n1"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = admin_call(
        &c.nodes[0],
        "POST",
        "/api/v1/cluster/shards/0/move",
        Some("nope"),
        Some(serde_json::json!({"to": "n1"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = admin_call(
        &c.nodes[0],
        "GET",
        "/api/v1/cluster/audit",
        Some(ADMIN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moving_a_shard_pins_it_and_unpinning_releases_the_pin() {
    let c = cluster(3).await;
    let shard = valka_core::ShardId(5);
    let from = c.nodes.iter().position(|n| n.engine.owns(shard)).unwrap();
    let to = (from + 1) % 3;
    let target = format!("n{to}");
    let (s, body) = admin_call(
        &c.nodes[(from + 2) % 3],
        "POST",
        "/api/v1/cluster/shards/5/move",
        Some(ADMIN),
        Some(serde_json::json!({ "to": target })),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{body}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !c.nodes[to].engine.owns(shard) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the shard never moved"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!c.nodes[from].engine.owns(shard));
    let (_, placement) = call(&c.nodes[0], get("/api/v1/cluster/placement")).await;
    assert_eq!(placement["pins"]["5"], target);
    assert_eq!(placement["effective_protocol"], 2);
    let (s, _) = admin_call(
        &c.nodes[0],
        "POST",
        "/api/v1/cluster/shards/5/move",
        Some(ADMIN),
        Some(serde_json::json!({"to": "ghost"})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "only eligible nodes");

    let (s, _) = admin_call(
        &c.nodes[0],
        "DELETE",
        "/api/v1/cluster/shards/5/pin",
        Some(ADMIN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, placement) = call(&c.nodes[0], get("/api/v1/cluster/placement")).await;
    assert!(placement["pins"].as_object().unwrap().is_empty());
    let (_, audit) = admin_call(
        &c.nodes[1],
        "GET",
        "/api/v1/cluster/audit",
        Some(ADMIN),
        None,
    )
    .await;
    let actions: Vec<&str> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, vec!["unpin_shard", "move_shard"], "newest first");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changing_partitions_reaches_every_node() {
    let c = cluster(2).await;
    let (s, _) = call(
        &c.nodes[0],
        post(
            "/api/v1/tasks",
            serde_json::json!({"queue_name": "grow", "task_name": "t"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, body) = admin_call(
        &c.nodes[1],
        "PUT",
        "/api/v1/queues/grow/partitions",
        Some(ADMIN),
        Some(serde_json::json!({"partitions": 4})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["partitions"], 4);
    assert_eq!(body["previous"], 16);
    assert_eq!(body["nodes_not_told"], serde_json::json!([]));
    for n in &c.nodes {
        assert_eq!(n.engine.queue_config("grow").await.unwrap().partitions, 4);
    }
    let (_, queues) = call(&c.nodes[0], get("/api/v1/queues")).await;
    let grow = queues
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["queue"] == "grow")
        .unwrap();
    // A shrink: the 12 removed partitions take no new tasks but stay served until empty.
    let rows = grow["partition_shards"].as_array().unwrap();
    assert_eq!(rows.len(), 16);
    assert_eq!(rows.iter().filter(|r| r["draining"] == true).count(), 12);
    assert_eq!(grow["served_partitions"], 16);
    let (s, _) = admin_call(
        &c.nodes[1],
        "PUT",
        "/api/v1/queues/grow/partitions",
        Some(ADMIN),
        Some(serde_json::json!({"partitions": 0})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // Queue names become object keys: a bad one is refused before anything is written.
    let (s, body) = admin_call(
        &c.nodes[1],
        "PUT",
        "/api/v1/queues/bad$q/partitions",
        Some(ADMIN),
        Some(serde_json::json!({"partitions": 4})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("queue_name may only contain"),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn draining_a_node_through_another_hands_its_shards_over() {
    let c = cluster(3).await;
    let mut ids = Vec::new();
    for i in 0..15 {
        let (s, b) = call(
            &c.nodes[i % 3],
            post(
                "/api/v1/tasks",
                serde_json::json!({"queue_name": "d", "task_name": "t"}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        ids.push(b["id"].as_str().unwrap().to_string());
    }
    let (s, body) = admin_call(
        &c.nodes[0],
        "POST",
        "/api/v1/cluster/nodes/n2/drain",
        Some(ADMIN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{body}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let survivors: usize = c.nodes[..2]
            .iter()
            .map(|n| n.engine.owned_shards().len())
            .sum();
        if c.nodes[2].engine.is_drained() && survivors == SHARDS as usize {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "drain did not complete"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(c.nodes[2].engine.owned_shards().is_empty());
    for id in &ids {
        let (s, _) = call(&c.nodes[0], get(&format!("/api/v1/tasks/{id}"))).await;
        assert_eq!(s, StatusCode::OK, "task {id} after the drain");
    }
    let (s, _) = admin_call(
        &c.nodes[0],
        "POST",
        "/api/v1/cluster/nodes/ghost/drain",
        Some(ADMIN),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}
