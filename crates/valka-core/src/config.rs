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
    pub database_url: String,
    pub web_dir: String,
    pub gossip: GossipConfig,
    pub matching: MatchingConfig,
    pub scheduler: SchedulerConfig,
    pub log_ingester: LogIngesterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipConfig {
    pub listen_addr: String,
    pub seed_nodes: Vec<String>,
    pub cluster_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchingConfig {
    pub num_partitions: i32,
    pub branching_factor: usize,
    pub max_buffer_per_partition: usize,
    pub task_reader_batch_size: i64,
    pub task_reader_poll_busy_ms: u64,
    pub task_reader_poll_idle_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    pub reaper_interval_secs: u64,
    pub lease_timeout_secs: i64,
    pub retry_base_delay_secs: u64,
    pub retry_max_delay_secs: u64,
    pub dlq_check_interval_secs: u64,
    pub delayed_check_interval_secs: u64,
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
            database_url: "postgresql://valka:valka@localhost:5432/valka".to_string(),
            web_dir: "web/dist".to_string(),
            gossip: GossipConfig::default(),
            matching: MatchingConfig::default(),
            scheduler: SchedulerConfig::default(),
            log_ingester: LogIngesterConfig::default(),
        }
    }
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:7280".to_string(),
            seed_nodes: vec![],
            cluster_id: "valka".to_string(),
        }
    }
}

impl Default for MatchingConfig {
    fn default() -> Self {
        Self {
            num_partitions: 4,
            branching_factor: 3,
            max_buffer_per_partition: 1000,
            task_reader_batch_size: 50,
            task_reader_poll_busy_ms: 10,
            task_reader_poll_idle_ms: 200,
        }
    }
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            reaper_interval_secs: 10,
            lease_timeout_secs: 60,
            retry_base_delay_secs: 1,
            retry_max_delay_secs: 3600,
            dlq_check_interval_secs: 30,
            delayed_check_interval_secs: 5,
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
    pub fn load(config_path: Option<&str>) -> Result<Self, figment::Error> {
        let mut figment = Figment::from(Serialized::defaults(ServerConfig::default()));

        if let Some(path) = config_path {
            figment = figment.merge(Toml::file(path));
        }

        figment = figment.merge(Env::prefixed("VALKA_").split("__"));

        figment.extract()
    }
}
