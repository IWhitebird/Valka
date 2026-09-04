use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use tokio::sync::{broadcast, mpsc};
use valka_cluster::{ClusterManager, NodeForwarder};
use valka_core::{MatchingConfig, NodeId, TaskStatus, WorkerId};
use valka_dispatcher::DispatcherService;
use valka_dispatcher::worker_handle::WorkerHandle;
use valka_engine::{CreateTask, Engine, EngineConfig, LogIngester, TaskView};
use valka_matching::{MatchingService, MatchingSink};
use valka_proto::WorkerResponse;
use valka_wal::Store;
use valka_wal::logstore::LogLine;

/// A fully wired single node on an in-memory (or provided) store.
pub struct TestNode {
    pub store: Store,
    pub engine: Engine,
    pub matching: MatchingService,
    pub dispatcher: DispatcherService,
    pub logs: Arc<LogIngester>,
    pub log_tx: mpsc::Sender<LogLine>,
    pub event_tx: broadcast::Sender<valka_proto::TaskEvent>,
    pub cluster: Arc<ClusterManager>,
    pub forwarder: NodeForwarder,
    pub node_id: NodeId,
}

impl TestNode {
    pub async fn new() -> Self {
        Self::on_store(Store::memory(), "test-node").await
    }

    /// Build (or rebuild after a simulated crash) a node on an existing store.
    pub async fn on_store(store: Store, node_id: &str) -> Self {
        let matching = MatchingService::new(MatchingConfig::default());
        let mut cfg = EngineConfig::for_tests(node_id);
        cfg.trust_self = false; // exercise the real assignment/ownership path
        let engine = Engine::open_with(
            store.clone(),
            cfg,
            valka_engine::TokioClock::new(),
            Arc::new(MatchingSink::new(matching.clone())),
        )
        .await
        .expect("engine open");
        let (event_tx, _) = broadcast::channel::<valka_proto::TaskEvent>(1024);
        valka_server::convert::spawn_event_bridge(&engine, event_tx.clone());

        let (log_tx, log_rx) = mpsc::channel(1024);
        let logs = LogIngester::new(store.clone(), 100, Duration::from_millis(20));
        let (_stx, srx) = tokio::sync::watch::channel(false);
        tokio::spawn(logs.clone().run(log_rx, srx));
        std::mem::forget(_stx);

        let node = NodeId(node_id.to_string());
        let dispatcher = DispatcherService::new(
            matching.clone(),
            engine.clone(),
            node.clone(),
            log_tx.clone(),
        );
        let cluster = Arc::new(ClusterManager::new_single_node(
            node.clone(),
            matching.config().num_partitions,
        ));
        Self {
            store,
            engine,
            matching,
            dispatcher,
            logs,
            log_tx,
            event_tx,
            cluster,
            forwarder: NodeForwarder::new(),
            node_id: node,
        }
    }

    pub fn router(&self) -> Router {
        let metrics_handle = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        valka_server::rest::build_api_router(
            self.engine.clone(),
            self.event_tx.clone(),
            self.dispatcher.clone(),
            self.logs.clone(),
            metrics_handle,
            self.cluster.clone(),
            self.forwarder.clone(),
        )
    }

    /// Create a task straight through the engine.
    pub async fn create(&self, queue: &str, name: &str) -> TaskView {
        self.engine
            .create_task(task_req(queue, name))
            .await
            .expect("create task")
    }

    pub async fn create_with(&self, req: CreateTask) -> TaskView {
        self.engine.create_task(req).await.expect("create task")
    }

    /// Move a task to RUNNING via a real dispatch record. Returns the run id.
    pub fn start(&self, task_id: &str, worker: &str) -> String {
        self.engine
            .dispatch(task_id, worker)
            .expect("dispatch")
            .run_id
    }

    pub async fn complete(&self, task_id: &str) -> TaskView {
        let run = self.start(task_id, "w");
        self.engine
            .complete_run(task_id, &run, None)
            .await
            .expect("complete")
    }

    pub async fn fail_terminal(&self, task_id: &str) -> TaskView {
        let run = self.start(task_id, "w");
        self.engine
            .fail_run(task_id, &run, "boom", false)
            .await
            .expect("fail");
        self.engine.get_task(task_id).unwrap()
    }

    pub async fn dead_letter(&self, task_id: &str) -> TaskView {
        let t = self.engine.get_task(task_id).unwrap();
        let mut view = t;
        while view.status != TaskStatus::DeadLetter {
            match view.status {
                TaskStatus::Pending => {
                    let run = self.start(task_id, "w");
                    self.engine
                        .fail_run(task_id, &run, "boom", true)
                        .await
                        .expect("fail");
                }
                TaskStatus::Retry => {
                    // promote by advancing paused time past the backoff
                    tokio::time::advance(Duration::from_secs(4000)).await;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => panic!("unexpected status {other}"),
            }
            view = self.engine.get_task(task_id).unwrap();
        }
        view
    }

    /// Register a fake worker with the dispatcher; returns its id and response stream.
    pub async fn register_worker(
        &self,
        queues: &[&str],
        concurrency: i32,
    ) -> (WorkerId, mpsc::Receiver<WorkerResponse>) {
        let (tx, rx) = mpsc::channel::<WorkerResponse>(64);
        let id = WorkerId::new();
        let handle = WorkerHandle::new(
            id.clone(),
            "test-worker".to_string(),
            queues.iter().map(|s| s.to_string()).collect(),
            concurrency,
            tx,
            String::new(),
        );
        self.dispatcher.register_worker(handle).await;
        (id, rx)
    }
}

pub fn task_req(queue: &str, name: &str) -> CreateTask {
    CreateTask {
        queue_name: queue.to_string(),
        task_name: name.to_string(),
        input: Some(serde_json::json!({"key": "value"})),
        priority: 0,
        max_retries: 3,
        timeout_seconds: 300,
        idempotency_key: None,
        metadata: serde_json::json!({}),
        scheduled_at: None,
    }
}

pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(60)).await;
}

pub fn json_body(value: serde_json::Value) -> String {
    serde_json::to_string(&value).unwrap()
}

pub async fn parse_response_json(
    response: axum::http::Response<axum::body::Body>,
) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

pub async fn assert_error_response(
    response: axum::http::Response<axum::body::Body>,
    expected_status: StatusCode,
    expected_code: &str,
    message_contains: &str,
) {
    assert_eq!(response.status(), expected_status);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&body).expect("Error response should be valid JSON");
    assert_eq!(
        json["code"].as_str().unwrap(),
        expected_code,
        "Expected error code {expected_code}, got {:?}",
        json["code"]
    );
    let error_msg = json["error"].as_str().unwrap();
    assert!(
        error_msg.contains(message_contains),
        "Expected error message to contain '{message_contains}', got '{error_msg}'"
    );
}
