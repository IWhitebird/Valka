use metrics::{counter, gauge, histogram};

pub fn record_task_created(queue: &str) {
    counter!("valka_tasks_created_total", "queue" => queue.to_string()).increment(1);
}

pub fn record_task_completed(queue: &str) {
    counter!("valka_tasks_completed_total", "queue" => queue.to_string()).increment(1);
}

pub fn record_task_failed(queue: &str) {
    counter!("valka_tasks_failed_total", "queue" => queue.to_string()).increment(1);
}

pub fn record_task_retried(queue: &str) {
    counter!("valka_tasks_retried_total", "queue" => queue.to_string()).increment(1);
}

pub fn record_task_dead_lettered(queue: &str) {
    counter!("valka_tasks_dead_lettered_total", "queue" => queue.to_string()).increment(1);
}

pub fn record_dispatch_latency(queue: &str, latency_ms: f64) {
    histogram!("valka_dispatch_latency_ms", "queue" => queue.to_string()).record(latency_ms);
}

pub fn record_task_duration(queue: &str, duration_ms: f64) {
    histogram!("valka_task_duration_ms", "queue" => queue.to_string()).record(duration_ms);
}

pub fn set_active_workers(count: f64) {
    gauge!("valka_active_workers").set(count);
}

pub fn set_pending_tasks(queue: &str, count: f64) {
    gauge!("valka_pending_tasks", "queue" => queue.to_string()).set(count);
}

pub fn record_sync_match() {
    counter!("valka_sync_matches_total").increment(1);
}

pub fn record_async_match() {
    counter!("valka_async_matches_total").increment(1);
}

pub fn set_cluster_members(count: f64) {
    gauge!("valka_cluster_members").set(count);
}

pub fn record_task_forwarded(queue: &str) {
    counter!("valka_tasks_forwarded_total", "queue" => queue.to_string()).increment(1);
}

pub fn record_forward_circuit_open(addr: &str) {
    counter!("valka_forward_circuit_open_total", "addr" => addr.to_string()).increment(1);
}

// ── cluster (PHASE2 M8) ──

/// Tasks this node holds in RAM (live and recently finished), against `limits.max_tasks_in_ram`.
pub fn set_tasks_in_ram(count: f64) {
    gauge!("valka_tasks_in_ram").set(count);
}

pub fn set_shards_owned(count: f64) {
    gauge!("valka_shards_owned").set(count);
}

pub fn set_lease_age(seconds: f64) {
    gauge!("valka_lease_age_seconds").set(seconds);
}

pub fn set_serving(serving: bool) {
    gauge!("valka_node_serving").set(if serving { 1.0 } else { 0.0 });
}

pub fn record_fenced() {
    counter!("valka_node_fenced_total").increment(1);
}

pub fn record_shard_claim(result: &'static str) {
    counter!("valka_shard_claims_total", "result" => result).increment(1);
}

pub fn record_shard_release(result: &'static str) {
    counter!("valka_shard_releases_total", "result" => result).increment(1);
}

pub fn record_tombstone() {
    counter!("valka_tombstones_total").increment(1);
}

pub fn record_split(records: usize, seconds: f64) {
    counter!("valka_splits_total").increment(1);
    counter!("valka_split_records_total").increment(records as u64);
    histogram!("valka_split_duration_seconds").record(seconds);
}

pub fn record_split_failed() {
    counter!("valka_split_failures_total").increment(1);
}

pub fn record_forwarded(command: &'static str, result: &'static str) {
    counter!("valka_forwarded_commands_total", "command" => command, "result" => result)
        .increment(1);
}

pub fn record_route_redirect() {
    counter!("valka_route_redirects_total").increment(1);
}

pub fn record_remote_fetch(tasks: usize) {
    counter!("valka_remote_fetches_total").increment(1);
    counter!("valka_remote_fetched_tasks_total").increment(tasks as u64);
}

pub fn record_reattach(outcome: &'static str) {
    counter!("valka_reattach_total", "outcome" => outcome).increment(1);
}

pub fn record_wal_segment(bytes: u64) {
    counter!("valka_wal_segments_total").increment(1);
    counter!("valka_wal_bytes_total").increment(bytes);
}

pub fn record_admin_action(action: &'static str) {
    counter!("valka_admin_actions_total", "action" => action).increment(1);
}
