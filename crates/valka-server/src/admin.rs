//! The admin API: drain a node, pin (move) a shard, change a queue's partition count.
//!
//! Every endpoint here needs `Authorization: Bearer <admin_token>` (`VALKA_ADMIN_TOKEN`);
//! without a configured token the admin API is disabled. Every action is written to
//! `admin/{ts}-{id}.json` in the bucket as an audit record.

use axum::{
    Json,
    extract::{Path, Query, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use valka_cluster::{Command, Reply};
use valka_wal::lsn::keys;

use crate::rest::{ApiError, AppState};

/// Reject requests without the admin token.
pub async fn require_admin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.admin_token.as_deref().filter(|t| !t.is_empty()) else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "admin API disabled: set VALKA_ADMIN_TOKEN",
                "code": "FORBIDDEN",
            })),
        )
            .into_response();
    };
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !valka_cluster::security::constant_time_eq(given.as_bytes(), expected.as_bytes()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "bad or missing admin token", "code": "UNAUTHORIZED"})),
        )
            .into_response();
    }
    next.run(req).await
}

async fn audit(state: &AppState, action: &'static str, params: Value, result: &Value) {
    valka_core::metrics::record_admin_action(action);
    let id = uuid::Uuid::now_v7().to_string();
    let now = chrono::Utc::now();
    let record = json!({
        "id": id,
        "action": action,
        "params": params,
        "result": result,
        "node": state.engine.node_id(),
        "at": now.to_rfc3339(),
    });
    let key = keys::admin_audit(now.timestamp_millis(), &id);
    if let Err(e) = state
        .engine
        .store()
        .put(&key, Bytes::from(record.to_string()))
        .await
    {
        tracing::warn!(action, error = %e, "writing the admin audit record failed");
    }
}

/// `POST /api/v1/cluster/nodes/{id}/drain`: the node hands every shard over (final
/// snapshots, nothing to replay), marks its lease dead and exits.
pub async fn drain_node(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let known =
        node == state.engine.node_id() || state.engine.live_peers().iter().any(|(n, _)| *n == node);
    if !known {
        return Err(ApiError::NotFound(format!("no live node {node}")));
    }
    let ok = state.api.send_to_node(&node, Command::Drain).await == Some(Reply::Done);
    let result = json!({ "accepted": ok });
    audit(&state, "drain_node", json!({ "node": node }), &result).await;
    if !ok {
        return Err(ApiError::Unavailable(format!(
            "{node} did not accept the drain"
        )));
    }
    Ok((StatusCode::ACCEPTED, Json(result)))
}

#[derive(Deserialize)]
pub struct MoveBody {
    to: String,
}

/// `POST /api/v1/cluster/shards/{id}/move` `{"to": node}`: pin the shard to a node. Its
/// owner hands it over on its next placement round; the pin holds while the node is
/// eligible (rendezvous placement decides again otherwise).
pub async fn move_shard(
    State(state): State<AppState>,
    Path(shard): Path<u16>,
    Json(body): Json<MoveBody>,
) -> Result<impl IntoResponse, ApiError> {
    if shard as u32 >= state.engine.num_shards() {
        return Err(ApiError::NotFound(format!("shard {shard} out of range")));
    }
    let eligible = state.engine.eligible_nodes();
    if !eligible.contains(&body.to) {
        return Err(ApiError::BadRequest(format!(
            "{} is not an eligible node (live, alive, past its join delay): {eligible:?}",
            body.to
        )));
    }
    let to = body.to.clone();
    let p = valka_wal::placement::update(state.engine.store(), move |p| {
        p.pins.insert(shard, to.clone());
    })
    .await?;
    state.engine.refresh_placement().await;
    let result = json!({ "shard": shard, "pinned_to": body.to, "placement_version": p.version });
    audit(
        &state,
        "move_shard",
        json!({ "shard": shard, "to": body.to }),
        &result,
    )
    .await;
    Ok((StatusCode::ACCEPTED, Json(result)))
}

/// `DELETE /api/v1/cluster/shards/{id}/pin`: back to rendezvous placement.
pub async fn unpin_shard(
    State(state): State<AppState>,
    Path(shard): Path<u16>,
) -> Result<impl IntoResponse, ApiError> {
    let p = valka_wal::placement::update(state.engine.store(), move |p| {
        p.pins.remove(&shard);
    })
    .await?;
    state.engine.refresh_placement().await;
    let result = json!({ "shard": shard, "placement_version": p.version });
    audit(&state, "unpin_shard", json!({ "shard": shard }), &result).await;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct PartitionsBody {
    partitions: u32,
}

/// `PUT /api/v1/queues/{queue}/partitions` `{"partitions": n}`. Growing is online (new
/// partitions take new tasks, old ones drain); keyed creates check the key's old partition
/// while the change is recent, so idempotency stays exact.
pub async fn set_partitions(
    State(state): State<AppState>,
    Path(queue): Path<String>,
    Json(body): Json<PartitionsBody>,
) -> Result<impl IntoResponse, ApiError> {
    let cfg = valka_wal::queues::set_partitions(
        state.engine.store(),
        &queue,
        body.partitions,
        valka_core::DEFAULT_QUEUE_PARTITIONS,
    )
    .await
    .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let missed = state
        .api
        .broadcast(Command::InvalidateQueue {
            queue: queue.clone(),
        })
        .await;
    let result = json!({
        "queue": cfg.queue,
        "partitions": cfg.partitions,
        "previous": cfg.previous.map(|p| p.partitions),
        "version": cfg.version,
        "nodes_not_told": missed,
    });
    audit(
        &state,
        "set_partitions",
        json!({ "queue": queue, "partitions": body.partitions }),
        &result,
    )
    .await;
    Ok(Json(result))
}

/// `GET /api/v1/queues`: every queue's partition config, and its shards' owners.
pub async fn list_queues(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let n = state.engine.num_shards();
    let eligible = state.engine.eligible_nodes();
    let rows: Vec<Value> = valka_wal::queues::list(state.engine.store())
        .await?
        .into_iter()
        .map(|c| {
            let partitions: Vec<Value> = (0..c.served_partitions())
                .map(|p| {
                    let shard = valka_core::partition_shard(&c.queue, p, n);
                    json!({
                        "partition": p,
                        "shard": shard.0,
                        "desired_owner": state.engine.desired_owner(shard, &eligible),
                        // Removed by a shrink: no new tasks, served until empty.
                        "draining": p >= c.partitions,
                    })
                })
                .collect();
            json!({
                "queue": c.queue,
                "partitions": c.partitions,
                "served_partitions": c.served_partitions(),
                "previous": c.previous,
                "version": c.version,
                "created_at": c.created_at,
                "partition_shards": partitions,
            })
        })
        .collect();
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_audit_limit")]
    limit: usize,
}

fn default_audit_limit() -> usize {
    50
}

/// `GET /api/v1/cluster/audit`: the newest admin actions.
pub async fn audit_log(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let store = state.engine.store();
    let mut keys: Vec<String> = store
        .list("admin/")
        .await?
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    keys.sort();
    let mut out = Vec::new();
    for k in keys.iter().rev().take(q.limit.clamp(1, 500)) {
        if let Some((bytes, _)) = store.get(k).await?
            && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
        {
            out.push(v);
        }
    }
    Ok(Json(out))
}

/// `GET /api/v1/cluster/placement`: operator pins and the effective protocol.
pub async fn placement(State(state): State<AppState>) -> impl IntoResponse {
    Json(json!({
        "pins": state.engine.pins(),
        "effective_protocol": state.engine.effective_protocol(),
        "eligible_nodes": state.engine.eligible_nodes(),
    }))
}
