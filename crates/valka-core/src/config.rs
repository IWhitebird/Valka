use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub node_id: String,
    pub grpc_addr: String,
    pub http_addr: String,
    pub web_dir: String,
    pub storage: StorageConfig,
    pub wal: WalConfig,
    pub gossip: GossipConfig,
    pub matching: MatchingConfig,
    pub scheduler: SchedulerConfig,
    pub log_ingester: LogIngesterConfig,
}

/// Where the WAL, snapshots and logs live. The bucket is the only durable state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// `memory` (tests), `local` (a directory), or `s3` (AWS S3 / MinIO / any S3-compatible).
    pub backend: String,
    /// Directory for the `local` backend.
    pub path: String,
    /// Bucket name for the `s3` backend.
    pub bucket: String,
    /// Optional key prefix inside the bucket (e.g. `valka/prod`).
    pub prefix: String,
    /// Custom endpoint for S3-compatible stores (MinIO: `http://localhost:9000`).
    pub endpoint: Option<String>,
    pub region: Option<String>,
    /// Credentials. When unset, the AWS default chain (env, profile, IMDS) is used.
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    /// Allow plain-HTTP endpoints (MinIO in dev).
    pub allow_http: bool,
}

/// WAL writer / snapshot knobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    /// Group-commit window. Nothing is acked before its segment is written.
    pub flush_interval_ms: u64,
    /// Flush early once the buffer reaches this many bytes.
    pub max_batch_bytes: usize,
    /// How many segment PUTs may be in flight concurrently.
    pub max_inflight_segments: usize,
    /// PUT retry attempts before parked acks fail with UNAVAILABLE.
    pub put_retries: u32,
    /// Snapshot dirty shards at least this often.
    pub snapshot_interval_secs: u64,
    /// Snapshot a shard once it has this many records since its last snapshot.
    pub snapshot_after_records: u64,
    /// Snapshot every dirty shard once this many WAL bytes have been committed since the
    /// last full round. Bounds restart replay (and, with many nodes, takeover). 0 disables.
    pub log_budget_bytes: u64,
    /// Snapshots older than the newest N per shard are deleted.
    pub snapshots_to_keep: usize,
    /// Terminal tasks (COMPLETED/FAILED/CANCELLED/DEAD_LETTER) are dropped from RAM after this.
    pub completed_retention_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipConfig {
    pub listen_addr: String,
    pub seed_nodes: Vec<String>,
    pub cluster_id: String,
    pub advertise_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchingConfig {
    pub num_partitions: i32,
    pub branching_factor: usize,
    pub max_buffer_per_partition: usize,
    /// How often the feeder tops up matching buffers from the engine's pending heaps.
    pub feeder_interval_ms: u64,
    /// Max tasks moved per queue per feeder tick.
    pub feeder_batch_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Timer wheel resolution (lease expiry, retry promotion, delayed tasks).
    pub timer_tick_ms: u64,
    /// Extra time granted on top of a task's timeout before its lease expires.
    pub lease_grace_secs: i64,
    /// Lease granted to RUNNING tasks recovered from the WAL until the worker re-handshakes.
    pub recovery_grace_secs: i64,
    /// Lease granted from each heartbeat: the worker has this long to heartbeat again.
    pub heartbeat_lease_secs: i64,
    pub retry_base_delay_secs: u64,
    pub retry_max_delay_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogIngesterConfig {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            node_id: String::new(),
            grpc_addr: "0.0.0.0:50051".to_string(),
            http_addr: "0.0.0.0:8989".to_string(),
            web_dir: "web/dist".to_string(),
            storage: StorageConfig::default(),
            wal: WalConfig::default(),
            gossip: GossipConfig::default(),
            matching: MatchingConfig::default(),
            scheduler: SchedulerConfig::default(),
            log_ingester: LogIngesterConfig::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "local".to_string(),
            path: "./data".to_string(),
            bucket: "valka".to_string(),
            prefix: String::new(),
            endpoint: None,
            region: None,
            access_key_id: None,
            secret_access_key: None,
            allow_http: false,
        }
    }
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            flush_interval_ms: 50,
            max_batch_bytes: 4 * 1024 * 1024,
            max_inflight_segments: 4,
            put_retries: 8,
            snapshot_interval_secs: 60,
            snapshot_after_records: 50_000,
            log_budget_bytes: 64 * 1024 * 1024,
            snapshots_to_keep: 2,
            completed_retention_secs: 24 * 3600,
        }
    }
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:7280".to_string(),
            seed_nodes: vec![],
            cluster_id: "valka".to_string(),
            advertise_addr: None,
        }
    }
}

impl Default for MatchingConfig {
    fn default() -> Self {
        Self {
            num_partitions: 4,
            branching_factor: 3,
            max_buffer_per_partition: 1000,
            feeder_interval_ms: 20,
            feeder_batch_size: 200,
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            timer_tick_ms: 100,
            lease_grace_secs: 30,
            recovery_grace_secs: 60,
            heartbeat_lease_secs: 60,
            retry_base_delay_secs: 1,
            retry_max_delay_secs: 3600,
        }
    }
}

impl Default for LogIngesterConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            flush_interval_ms: 500,
        }
    }
}

impl ServerConfig {
    pub fn load(config_path: Option<&str>) -> Result<Self, Box<figment::Error>> {
        let mut figment = Figment::from(Serialized::defaults(ServerConfig::default()));

        if let Some(path) = config_path {
            figment = figment.merge(Toml::file(path));
        }

        figment = figment.merge(Env::prefixed("VALKA_").split("__"));

        figment.extract().map_err(Box::new)
    }
}
