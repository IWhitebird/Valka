//! Operator-facing cluster endpoints: node health, shard map, bucket storage.
//!
//! Node stats and the shard map are gathered from every live node; membership (including
//! dead and suspect nodes) comes from this node's view of the leases in the bucket.

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
use valka_core::ShardId;
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

fn health(
    stats: &[NodeStats],
    members: &[valka_engine::MemberInfo],
    num_shards: u32,
    partial: bool,
) -> Value {
    let owned: usize = stats.iter().map(|s| s.shards_owned).sum();
    // Only a complete answer can count unowned shards.
    let unowned = (!partial).then(|| (num_shards as usize).saturating_sub(owned));
    let poisoned: Vec<&str> = stats
        .iter()
        .filter(|s| s.wal.poisoned.is_some())
        .map(|s| s.node_id.as_str())
        .collect();
    let fenced = stats.iter().any(|s| !s.serving);
    let suspect: Vec<&str> = members
        .iter()
        .filter(|m| m.suspect)
        .map(|m| m.node_id.as_str())
        .collect();
    let status = if !poisoned.is_empty() || fenced {
        "critical"
    } else if unowned.is_none_or(|u| u > 0) || !suspect.is_empty() {
        "degraded"
    } else {
        "ok"
    };
    json!({
        "status": status,
        "unowned_shards": unowned,
        "poisoned_nodes": poisoned,
        "suspect_nodes": suspect,
        "partial": partial,
    })
}

fn member_status(m: &valka_engine::MemberInfo) -> &'static str {
    if m.suspect {
        "suspect"
    } else if m.state == "dead" {
        "dead"
    } else if m.state == "draining" {
        "draining"
    } else if m.live {
        "alive"
    } else {
        "unknown"
    }
}

/// A node known only from its lease (dead, suspect, or not answering).
fn member_json(m: &valka_engine::MemberInfo) -> Value {
    json!({
        "node_id": m.node_id,
        "inc": m.inc,
        "status": member_status(m),
        "grpc_addr": m.grpc_addr,
        "http_addr": m.http_addr,
        "internal_addr": m.internal_addr,
        "version": m.version,
        "protocol": m.protocol,
        "started_at": m.started_at.map(|t| t.to_rfc3339()),
        "lease_age_ms": m.lease_age_ms,
        "eligible": m.eligible,
        "unsplit": m.unsplit,
        "dead_by": m.dead_by,
    })
}

/// A node that answered with its stats.
fn node_json(
    state: &AppState,
    stats: &NodeStats,
    member: Option<&valka_engine::MemberInfo>,
    workers: usize,
) -> Value {
    let me = stats.node_id == state.engine.node_id();
    json!({
        "node_id": stats.node_id,
        "inc": stats.wal.inc,
        "status": if stats.wal.poisoned.is_some() {
            "poisoned"
        } else if !stats.serving {
            "fenced"
        } else if member.is_some_and(|m| m.state == "draining") {
            "draining"
        } else {
            "alive"
        },
        "lease_age_ms": stats.lease_age_ms,
        // Advertised addresses (what clients use); this node's listen address if it has none.
        "grpc_addr": member
            .and_then(|m| m.grpc_addr.clone())
            .or_else(|| me.then(|| state.node_info.grpc_addr.clone())),
        "http_addr": member
            .and_then(|m| m.http_addr.clone())
            .or_else(|| me.then(|| state.node_info.http_addr.clone())),
        "version": member.map_or(state.node_info.version.clone(), |m| m.version.clone()),
        "started_at": stats.started_at.to_rfc3339(),
        "storage_backend": state.engine.store().backend_label(),
        "shards_owned": stats.shards_owned,
        "shards_with_tasks": stats.shards_with_tasks,
        "tasks": stats.tasks,
        "queues": stats.queues,
        "workers_connected": workers,
        "wal": stats.wal,
        "snapshots": stats.snapshots,
        "eligible": member.is_some_and(|m| m.eligible),
    })
}

pub async fn cluster_overview(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let stats = state.api.node_stats().await?;
    let workers = state.api.workers().await?;
    let members = state.engine.members();
    let mut nodes: Vec<Value> = stats
        .value
        .iter()
        .map(|s| {
            let m = members.iter().find(|m| m.node_id == s.node_id);
            let w = workers
                .value
                .iter()
                .filter(|w| w.node_id == s.node_id)
                .count();
            node_json(&state, s, m, w)
        })
        .collect();
    nodes.extend(
        members
            .iter()
            .filter(|m| !stats.value.iter().any(|s| s.node_id == m.node_id))
            .map(member_json),
    );
    let num_shards = state.engine.num_shards();
    Ok(Json(json!({
        "cluster_id": state.engine.cluster_meta().cluster_id,
        "this_node": state.engine.node_id(),
        "clustered": state.engine.config().cluster.enabled,
        "num_shards": num_shards,
        "health": health(&stats.value, &members, num_shards, stats.partial()),
        "nodes": nodes,
    })))
}

/// This node's view of membership, straight from the bucket's leases.
pub async fn members(State(state): State<AppState>) -> impl IntoResponse {
    let rows: Vec<Value> = state
        .engine
        .members()
        .iter()
        .map(|m| {
            let mut v = member_json(m);
            v["live"] = json!(m.live);
            v["suspect"] = json!(m.suspect);
            v["state"] = json!(m.state);
            v
        })
        .collect();
    Json(rows)
}

#[derive(Deserialize)]
pub struct ShardsQuery {
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    dirty: Option<bool>,
    #[serde(default)]
    min_tasks: Option<usize>,
    /// Only shards nobody serves right now.
    #[serde(default)]
    unowned: Option<bool>,
}

pub async fn list_shards(
    State(state): State<AppState>,
    Query(q): Query<ShardsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    // Every node reports the shards it serves; the rest are unowned right now, as far as
    // a complete answer can tell: a node that did not answer makes it partial (its shards
    // show without an owner).
    let served = state.api.shard_map().await?;
    let missing = served.missing;
    let mut by_shard: std::collections::BTreeMap<u16, valka_engine::ShardStats> =
        served.value.into_iter().map(|s| (s.shard, s)).collect();
    let local = state.engine.shard_stats();
    let rows: Vec<_> = ShardId::all(state.engine.num_shards())
        .map(|s| {
            by_shard.remove(&s.0).unwrap_or_else(|| {
                let mut row = local[s.index()].clone();
                row.owner = None;
                row
            })
        })
        .filter(|s| {
            q.node
                .as_deref()
                .is_none_or(|n| s.owner.as_deref() == Some(n))
        })
        .filter(|s| !q.unowned.unwrap_or(false) || s.owner.is_none())
        .filter(|s| !q.dirty.unwrap_or(false) || s.records_since_snapshot > 0)
        .filter(|s| q.min_tasks.is_none_or(|m| s.tasks >= m))
        .collect();
    Ok((crate::rest::partial_headers(&missing), Json(rows)))
}

/// One shard, from the node serving it; a shard nobody serves right now is shown from
/// this node's (empty) view with no owner.
pub async fn get_shard(
    State(state): State<AppState>,
    Path(shard): Path<u16>,
) -> Result<impl IntoResponse, ApiError> {
    let id = ShardId(shard);
    let Some(mut local) = state.engine.shard_detail(id) else {
        return Err(ApiError::NotFound(format!(
            "shard {shard} out of range (0..{})",
            state.engine.num_shards()
        )));
    };
    if state.engine.owns(id) {
        return Ok(Json(local));
    }
    match state.api.shard_detail(id).await {
        Ok(d) => Ok(Json(d)),
        Err(_) => {
            local.stats.owner = None;
            Ok(Json(local))
        }
    }
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
