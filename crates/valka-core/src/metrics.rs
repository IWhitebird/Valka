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
