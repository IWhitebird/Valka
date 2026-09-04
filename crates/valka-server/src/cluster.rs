//! Operator-facing cluster endpoints: node health, shard map, bucket storage.
//!
//! Phase 1 reports the single node that owns every shard; the response shapes already
//! carry a node list so the UI does not change when ownership is distributed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use valka_core::{NUM_SHARDS, ShardId};
use valka_engine::{Engine, NodeStats};

use crate::rest::{ApiError, AppState};

/// Static facts about this process, shown on the node card.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub grpc_addr: String,
    pub http_addr: String,
    pub version: String,
    /// WAL group-commit window, used for the storage cost estimate.
    pub flush_interval_ms: u64,
}

impl NodeInfo {
    pub fn new(grpc_addr: &str, http_addr: &str, flush_interval_ms: u64) -> Self {
        Self {
            grpc_addr: grpc_addr.to_string(),
            http_addr: http_addr.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            flush_interval_ms,
        }
    }
}

/// Storage stats need one LIST per prefix; cache them briefly.
#[derive(Default)]
pub struct StorageCache(Mutex<Option<(Instant, Value)>>);

const STORAGE_CACHE_TTL: Duration = Duration::from_secs(30);
/// S3 Standard PUT price, USD per 1000 requests (us-east-1). Only for the estimate.
const S3_PUT_USD_PER_1000: f64 = 0.005;

fn health(stats: &NodeStats) -> Value {
    let unowned = NUM_SHARDS as usize - stats.shards_owned;
    let poisoned: Vec<&str> = stats
        .wal
        .poisoned
        .as_ref()
        .map(|_| stats.node_id.as_str())
        .into_iter()
        .collect();
    let status = if !poisoned.is_empty() {
        "critical"
    } else if unowned > 0 {
        "degraded"
    } else {
        "ok"
    };
    json!({
        "status": status,
        "unowned_shards": unowned,
        "poisoned_nodes": poisoned,
        "suspect_nodes": [],
    })
}

fn node_json(state: &AppState, stats: &NodeStats) -> Value {
    json!({
        "node_id": stats.node_id,
        "epoch": stats.wal.epoch,
        "status": if stats.wal.poisoned.is_some() { "poisoned" } else { "alive" },
        "grpc_addr": state.node_info.grpc_addr,
        "http_addr": state.node_info.http_addr,
        "version": state.node_info.version,
        "started_at": stats.started_at.to_rfc3339(),
        "storage_backend": state.engine.store().backend_label(),
        "shards_owned": stats.shards_owned,
        "shards_with_tasks": stats.shards_with_tasks,
        "tasks": stats.tasks,
        "queues": stats.queues,
        "workers_connected": state.dispatcher.workers().len(),
        "wal": stats.wal,
        "snapshots": stats.snapshots,
    })
}

pub async fn cluster_overview(State(state): State<AppState>) -> impl IntoResponse {
    let stats = state.engine.stats();
    Json(json!({
        "cluster_id": state.cluster.node_id().0,
        "this_node": stats.node_id,
        "clustered": state.cluster.is_clustered(),
        "num_shards": NUM_SHARDS,
        "health": health(&stats),
        "nodes": [node_json(&state, &stats)],
    }))
}

#[derive(Deserialize)]
pub struct ShardsQuery {
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    dirty: Option<bool>,
    #[serde(default)]
    min_tasks: Option<usize>,
}

pub async fn list_shards(
    State(state): State<AppState>,
    Query(q): Query<ShardsQuery>,
) -> impl IntoResponse {
    let rows: Vec<_> = state
        .engine
        .shard_stats()
        .into_iter()
        .filter(|s| {
            q.node
                .as_deref()
                .is_none_or(|n| s.owner.as_deref() == Some(n))
        })
        .filter(|s| !q.dirty.unwrap_or(false) || s.records_since_snapshot > 0)
        .filter(|s| q.min_tasks.is_none_or(|m| s.tasks >= m))
        .collect();
    Json(rows)
}

pub async fn get_shard(
    State(state): State<AppState>,
    Path(shard): Path<u16>,
) -> Result<impl IntoResponse, ApiError> {
    state
        .engine
        .shard_detail(ShardId(shard))
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("shard {shard} out of range (0..{NUM_SHARDS})")))
}

pub async fn storage(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let mut cache = state.storage_cache.0.lock().await;
    if let Some((at, v)) = cache.as_ref()
        && at.elapsed() < STORAGE_CACHE_TTL
    {
        return Ok(Json(v.clone()));
    }
    let v = storage_stats(&state.engine, state.node_info.flush_interval_ms).await?;
    *cache = Some((Instant::now(), v.clone()));
    Ok(Json(v))
}

async fn storage_stats(engine: &Engine, flush_interval_ms: u64) -> Result<Value, ApiError> {
    let store = engine.store();
    let node = engine.node_id();
    let wal = store.stats(&format!("wal/{node}/")).await?;
    let snaps = store.stats("snapshots/").await?;
    let logs = store.stats("logs/").await?;
    let now = engine.clock().now();
    let age = |t: Option<chrono::DateTime<chrono::Utc>>| t.map(|t| (now - t).num_seconds().max(0));
    // Upper bound: one segment PUT per flush window while busy, plus one snapshot round.
    let puts_per_day = 86_400_000f64 / flush_interval_ms.max(1) as f64;
    Ok(json!({
        "backend": store.backend_label(),
        "wal": {
            "segments": wal.objects,
            "bytes": wal.bytes,
            "oldest_age_secs": age(wal.oldest),
            "newest_age_secs": age(wal.newest),
        },
        "snapshots": {
            "count": snaps.objects,
            "bytes": snaps.bytes,
            "oldest_age_secs": age(snaps.oldest),
        },
        "logs": { "chunks": logs.objects, "bytes": logs.bytes },
        "estimate": {
            "flush_interval_ms": flush_interval_ms,
            "max_puts_per_day": puts_per_day.round(),
            "max_usd_per_day": (puts_per_day / 1000.0 * S3_PUT_USD_PER_1000 * 100.0).round() / 100.0,
        },
        "computed_at": now.to_rfc3339(),
    }))
}

pub async fn snapshot_now(State(state): State<AppState>) -> impl IntoResponse {
    state.engine.snapshot_now().await;
    state.storage_cache.0.lock().await.take();
    let s = state.engine.stats();
    (
        StatusCode::OK,
        Json(json!({ "snapshots": s.snapshots, "durable_lsn": s.wal.durable_lsn })),
    )
}

pub fn new_storage_cache() -> Arc<StorageCache> {
    Arc::new(StorageCache::default())
}
