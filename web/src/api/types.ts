export type TaskStatus =
  | "PENDING"
  | "DISPATCHING"
  | "RUNNING"
  | "COMPLETED"
  | "FAILED"
  | "RETRY"
  | "DEAD_LETTER"
  | "CANCELLED";

export interface Task {
  id: string;
  queue_name: string;
  task_name: string;
  status: TaskStatus;
  priority: number;
  max_retries: number;
  attempt_count: number;
  timeout_seconds: number;
  idempotency_key: string | null;
  input: Record<string, unknown> | null;
  metadata: Record<string, unknown> | null;
  output: Record<string, unknown> | null;
  error_message: string | null;
  scheduled_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface TaskRun {
  id: string;
  task_id: string;
  attempt_number: number;
  status: string;
  worker_id: string | null;
  assigned_node_id: string | null;
  output: Record<string, unknown> | null;
  error_message: string | null;
  started_at: string;
  completed_at: string | null;
  lease_expires_at: string;
  last_heartbeat: string;
}

export interface TaskLog {
  id: number;
  task_run_id: string;
  level: string;
  message: string;
  timestamp_ms: number;
  metadata: Record<string, unknown> | null;
}

export interface Worker {
  id: string;
  node_id?: string;
  name: string;
  queues: string[];
  concurrency: number;
  active_tasks: number;
  status: string;
  last_heartbeat: string;
  connected_at: string;
}

export interface DeadLetter {
  id: number;
  task_id: string;
  queue_name: string;
  task_name: string;
  error_message: string | null;
  created_at: string;
  attempt_count: number;
  input: Record<string, unknown> | null;
  metadata: Record<string, unknown> | null;
}

// Raw SSE event from backend (numeric status)
export interface RawTaskEvent {
  event_id: string;
  task_id: string;
  queue_name: string;
  new_status: number;
  timestamp_ms: number;
}

// Parsed event for UI display
export interface TaskEvent {
  event_id: string;
  task_id: string;
  queue_name: string;
  status: TaskStatus;
  timestamp: string;
}

export interface CreateTaskRequest {
  queue_name: string;
  task_name: string;
  input?: Record<string, unknown>;
  priority?: number;
  max_retries?: number;
  timeout_seconds?: number;
  scheduled_at?: string;
  idempotency_key?: string;
  metadata?: Record<string, unknown>;
}

export interface ListTasksParams {
  queue_name?: string;
  status?: string;
  limit?: number;
  offset?: number;
}

export interface ListDeadLettersParams {
  queue_name?: string;
  limit?: number;
  offset?: number;
}

export type SignalStatus = "PENDING" | "DELIVERED" | "ACKNOWLEDGED";

export interface TaskSignal {
  id: string;
  task_id: string;
  signal_name: string;
  payload: Record<string, unknown> | null;
  status: SignalStatus;
  created_at: string;
  delivered_at: string | null;
  acknowledged_at: string | null;
}

export interface SendSignalRequest {
  signal_name: string;
  payload?: Record<string, unknown>;
}

export interface SendSignalResponse {
  signal_id: string;
  delivered: boolean;
}

// ─── Cluster ────────────────────────────────────────────────────────

export interface TaskCounts {
  pending: number;
  running: number;
  retry: number;
  completed: number;
  failed: number;
  dead_letter: number;
  cancelled: number;
  total: number;
}

export interface WalStats {
  epoch: number;
  durable_lsn: string;
  next_lsn: string;
  unflushed_records: number;
  oldest_unacked_ms: number | null;
  poisoned: string | null;
}

export interface SnapshotStats {
  last_round_at: string | null;
  dirty_shards: number;
  oldest_dirty_lsn: string | null;
  shards_with_snapshot: number;
}

export type NodeStatus = "alive" | "poisoned" | "suspect" | "dead" | "draining";

export interface ClusterNode {
  node_id: string;
  epoch: number;
  status: NodeStatus;
  grpc_addr: string;
  http_addr: string;
  version: string;
  started_at: string;
  storage_backend: string;
  shards_owned: number;
  shards_with_tasks: number;
  tasks: TaskCounts;
  queues: string[];
  workers_connected: number;
  wal: WalStats;
  snapshots: SnapshotStats;
}

export interface ClusterHealth {
  status: "ok" | "degraded" | "critical";
  unowned_shards: number;
  poisoned_nodes: string[];
  suspect_nodes: string[];
}

export interface ClusterOverview {
  cluster_id: string;
  this_node: string;
  clustered: boolean;
  num_shards: number;
  health: ClusterHealth;
  nodes: ClusterNode[];
}

export interface ShardStats {
  shard: number;
  owner: string | null;
  epoch: number;
  tasks: number;
  pending: number;
  running: number;
  retry: number;
  signals: number;
  dead_letters: number;
  shard_seq: number;
  snapshot_seq: number;
  snapshot_lsn: string | null;
  snapshot_at: string | null;
  records_since_snapshot: number;
  dirty_since_lsn: string | null;
}

export interface ShardDetail extends ShardStats {
  queues: Record<string, TaskCounts>;
}

export interface ListShardsParams {
  node?: string;
  dirty?: boolean;
  min_tasks?: number;
}

export interface StorageStats {
  backend: string;
  wal: {
    segments: number;
    bytes: number;
    oldest_age_secs: number | null;
    newest_age_secs: number | null;
  };
  snapshots: { count: number; bytes: number; oldest_age_secs: number | null };
  logs: { chunks: number; bytes: number };
  estimate: {
    flush_interval_ms: number;
    max_puts_per_day: number;
    max_usd_per_day: number;
  };
  computed_at: string;
}
