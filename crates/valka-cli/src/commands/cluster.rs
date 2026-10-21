use anyhow::Result;
use reqwest::Method;
use serde_json::{Value, json};

use super::{Auth, Conn};

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub async fn status(conn: &Conn) -> Result<()> {
    let o = conn
        .rest(Method::GET, "/api/v1/cluster", None, Auth::Api)
        .await?;
    let h = &o["health"];
    println!(
        "Cluster {}  health {}  shards {}  {}",
        s(&o["cluster_id"]),
        s(&h["status"]).to_uppercase(),
        s(&o["num_shards"]),
        if o["clustered"] == true {
            "clustered"
        } else {
            "single node"
        }
    );
    match h["unowned_shards"].as_u64() {
        Some(0) => {}
        Some(n) => println!("  {n} shards have no owner right now"),
        None => println!("  some nodes did not answer: ownership unknown"),
    }
    for (label, key) in [("poisoned", "poisoned_nodes"), ("suspect", "suspect_nodes")] {
        if let Some(list) = h[key].as_array().filter(|l| !l.is_empty()) {
            let names: Vec<String> = list.iter().map(s).collect();
            println!("  {label}: {}", names.join(", "));
        }
    }
    println!();
    println!(
        "{:<16} {:>4} {:<9} {:>7} {:>9} {:>8} {:>8}  GRPC",
        "NODE", "INC", "STATUS", "SHARDS", "PENDING", "RUNNING", "WORKERS"
    );
    for n in o["nodes"].as_array().into_iter().flatten() {
        println!(
            "{:<16} {:>4} {:<9} {:>7} {:>9} {:>8} {:>8}  {}",
            s(&n["node_id"]),
            s(&n["inc"]),
            s(&n["status"]),
            s(&n["shards_owned"]),
            s(&n["tasks"]["pending"]),
            s(&n["tasks"]["running"]),
            s(&n["workers_connected"]),
            s(&n["grpc_addr"]),
        );
    }
    Ok(())
}

pub async fn members(conn: &Conn) -> Result<()> {
    let rows = conn
        .rest(Method::GET, "/api/v1/cluster/members", None, Auth::Api)
        .await?;
    println!(
        "{:<16} {:>4} {:<9} {:>10} {:<9} {:>8}  UNSPLIT",
        "NODE", "INC", "STATUS", "LEASE AGE", "ELIGIBLE", "PROTOCOL"
    );
    for m in rows.as_array().into_iter().flatten() {
        let age = m["lease_age_ms"]
            .as_u64()
            .map(|ms| format!("{:.1} s", ms as f64 / 1000.0))
            .unwrap_or_else(|| "-".into());
        let unsplit: Vec<String> = m["unsplit"]
            .as_array()
            .into_iter()
            .flatten()
            .map(s)
            .collect();
        println!(
            "{:<16} {:>4} {:<9} {:>10} {:<9} {:>8}  {}",
            s(&m["node_id"]),
            s(&m["inc"]),
            s(&m["status"]),
            age,
            if m["eligible"] == true { "yes" } else { "no" },
            s(&m["protocol"]),
            if unsplit.is_empty() {
                "-".to_string()
            } else {
                unsplit.join(",")
            },
        );
    }
    Ok(())
}

pub async fn workers(conn: &Conn) -> Result<()> {
    let rows = conn
        .rest(Method::GET, "/api/v1/workers", None, Auth::Api)
        .await?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("No workers connected");
        return Ok(());
    }
    println!(
        "{:<38} {:<20} {:<14} {:>6} {:>11}  QUEUES",
        "ID", "NAME", "NODE", "ACTIVE", "CONCURRENCY"
    );
    for w in rows {
        let queues: Vec<String> = w["queues"]
            .as_array()
            .into_iter()
            .flatten()
            .map(s)
            .collect();
        println!(
            "{:<38} {:<20} {:<14} {:>6} {:>11}  {}",
            s(&w["id"]),
            s(&w["name"]),
            s(&w["node_id"]),
            s(&w["active_tasks"]),
            s(&w["concurrency"]),
            queues.join(","),
        );
    }
    Ok(())
}

pub async fn queues(conn: &Conn) -> Result<()> {
    let rows = conn
        .rest(Method::GET, "/api/v1/queues", None, Auth::Api)
        .await?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("No queues yet (a queue appears with its first task)");
        return Ok(());
    }
    println!(
        "{:<24} {:>10} {:>8} {:>8}",
        "QUEUE", "PARTITIONS", "SERVED", "VERSION"
    );
    for q in rows {
        println!(
            "{:<24} {:>10} {:>8} {:>8}",
            s(&q["queue"]),
            s(&q["partitions"]),
            s(&q["served_partitions"]),
            s(&q["version"]),
        );
    }
    Ok(())
}

pub async fn drain(conn: &Conn, node: &str) -> Result<()> {
    conn.rest(
        Method::POST,
        &format!("/api/v1/cluster/nodes/{node}/drain"),
        None,
        Auth::Admin,
    )
    .await?;
    println!("{node} is handing its shards over and will stop");
    Ok(())
}

pub async fn move_shard(conn: &Conn, shard: u16, to: &str) -> Result<()> {
    conn.rest(
        Method::POST,
        &format!("/api/v1/cluster/shards/{shard}/move"),
        Some(json!({ "to": to })),
        Auth::Admin,
    )
    .await?;
    println!("Shard {shard} is pinned to {to}; it moves on the next placement round");
    Ok(())
}

pub async fn unpin(conn: &Conn, shard: u16) -> Result<()> {
    conn.rest(
        Method::DELETE,
        &format!("/api/v1/cluster/shards/{shard}/pin"),
        None,
        Auth::Admin,
    )
    .await?;
    println!("Shard {shard} is back under automatic placement");
    Ok(())
}

pub async fn set_partitions(conn: &Conn, queue: &str, count: u32) -> Result<()> {
    let r = conn
        .rest(
            Method::PUT,
            &format!("/api/v1/queues/{queue}/partitions"),
            Some(json!({ "partitions": count })),
            Auth::Admin,
        )
        .await?;
    println!(
        "{queue}: {} partitions (was {})",
        s(&r["partitions"]),
        s(&r["previous"])
    );
    if let Some(missed) = r["nodes_not_told"].as_array().filter(|m| !m.is_empty()) {
        let names: Vec<String> = missed.iter().map(s).collect();
        println!(
            "  not told yet (they pick it up within half a minute): {}",
            names.join(", ")
        );
    }
    Ok(())
}
