use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub node_id: String,
    /// Bearer token for the admin API (drain, move shard, set partitions). Unset: the
    /// admin API is disabled.
    pub admin_token: Option<String>,
    pub grpc_addr: String,
    pub http_addr: String,
    pub web_dir: String,
    pub storage: StorageConfig,
    pub wal: WalConfig,
    pub cluster: ClusterConfig,
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

/// Membership, leases and shard placement. One node is a cluster of one: the same
/// protocol runs whether or not other nodes exist (docs/wal/PHASE2.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Allow other nodes on this bucket. Needs an S3-compatible backend (the local and
    /// memory backends only work inside one process) and a `token`.
    pub enabled: bool,
    /// Shard count for a new cluster, written to `cluster.json` on first start. A power of
    /// two in 16..=65536. A running cluster's count never changes; 0 means "whatever the
    /// bucket says, or the default for a new bucket".
    pub num_shards: u32,
    /// Shared secret every internal call carries. Required when `enabled`.
    pub token: Option<String>,
    /// Listener for node-to-node RPCs. Never expose it publicly.
    pub internal_addr: String,
    /// Address other nodes use to reach `internal_addr` (defaults to it).
    pub advertise_internal_addr: Option<String>,
    /// Address workers and clients use to reach this node's public gRPC port.
    pub advertise_grpc_addr: Option<String>,
    /// Address used to reach this node's REST port.
    pub advertise_http_addr: Option<String>,
    /// Optional mTLS for internal traffic (PEM files). The token is still required.
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub tls_ca: Option<String>,
    /// How often a node renews its lease.
    pub lease_interval_ms: u64,
    /// A lease unchanged for this long (by the observer's own clock) is dead.
    pub lease_timeout_ms: u64,
    /// A node that has not renewed for `lease_timeout - safety_margin` stops serving.
    pub lease_safety_margin_ms: u64,
    /// How often the bucket's node list is re-read.
    pub membership_interval_ms: u64,
    /// A newly seen node takes part in placement after this long.
    pub join_delay_ms: u64,
    /// How often each node re-evaluates which shards it should own.
    pub placement_interval_ms: u64,
    /// Shard claims in flight at once.
    pub claim_concurrency: usize,
    /// Shards a node hands over per placement tick.
    pub handover_per_tick: usize,
    /// A shard stays where it is for at least this long after a move.
    pub min_residency_secs: u64,
    /// Another node may redo a dead node's log split after this long.
    pub split_timeout_ms: u64,
    /// After claiming shards, recovered PENDING tasks wait this long before re-dispatch so
    /// reconnecting workers can reattach to their runs first.
    pub reconcile_grace_ms: u64,
    /// Garbage collector cadence (dead logs, old snapshots, split markers, dead leases).
    pub gc_interval_secs: u64,
    /// Refuse to start while another live process holds this node id.
    pub startup_lease_check: bool,
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
            admin_token: None,
            grpc_addr: "0.0.0.0:50051".to_string(),
            http_addr: "0.0.0.0:8989".to_string(),
            web_dir: "web/dist".to_string(),
            storage: StorageConfig::default(),
            wal: WalConfig::default(),
            cluster: ClusterConfig::default(),
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

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            num_shards: 0,
            token: None,
            internal_addr: "0.0.0.0:50052".to_string(),
            advertise_internal_addr: None,
            advertise_grpc_addr: None,
            advertise_http_addr: None,
            tls_cert: None,
            tls_key: None,
            tls_ca: None,
            lease_interval_ms: 2_000,
            lease_timeout_ms: 10_000,
            lease_safety_margin_ms: 3_000,
            membership_interval_ms: 3_000,
            join_delay_ms: 15_000,
            placement_interval_ms: 3_000,
            claim_concurrency: 64,
            handover_per_tick: 64,
            min_residency_secs: 300,
            split_timeout_ms: 30_000,
            reconcile_grace_ms: 5_000,
            gc_interval_secs: 300,
            startup_lease_check: true,
        }
    }
}

impl ClusterConfig {
    /// Settings for tests: fast placement, no join delay, and leases that never go stale
    /// on their own (tests that jump paused time by hours must not self-fence).
    pub fn for_tests() -> Self {
        Self {
            lease_interval_ms: 2_000,
            lease_timeout_ms: 24 * 3600 * 1000,
            lease_safety_margin_ms: 3_000,
            membership_interval_ms: 1_000,
            join_delay_ms: 0,
            placement_interval_ms: 500,
            min_residency_secs: 0,
            reconcile_grace_ms: 0,
            startup_lease_check: false,
            ..Default::default()
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
