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

export interface TaskCheckpoint {
  task_id: string;
  step: string;
  output: unknown;
  run_id: string;
  attempt_number: number;
  created_at: string;
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
  /** The node process's incarnation (bumped on every start). */
  inc: number;
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

/**
 * alive / draining: serving; fenced: lost its lease or too old to serve; poisoned: WAL writer
 * failed; suspect: lease not renewed lately; dead: tombstoned or exited; unknown: lease seen,
 * node not answering.
 */
export type NodeStatus =
  | "alive"
  | "draining"
  | "fenced"
  | "poisoned"
  | "suspect"
  | "dead"
  | "unknown";

/** What every node row carries, from the node itself or from its lease in the bucket. */
export interface ClusterNodeBase {
  node_id: string;
  inc: number;
  status: NodeStatus;
  grpc_addr: string | null;
  http_addr: string | null;
  version: string;
  started_at: string | null;
  lease_age_ms: number | null;
  eligible: boolean;
}

/** A node that answered with its stats. */
export interface ClusterNodeStats extends ClusterNodeBase {
  storage_backend: string;
  shards_owned: number;
  shards_with_tasks: number;
  tasks: TaskCounts;
  queues: string[];
  workers_connected: number;
  wal: WalStats;
  snapshots: SnapshotStats;
}

/** A node known only from its lease (dead, suspect, or not answering). */
export interface ClusterNodeLease extends ClusterNodeBase {
  internal_addr: string | null;
  protocol: number;
  unsplit: number[];
}

export type ClusterNode = ClusterNodeStats | ClusterNodeLease;

export function hasStats(node: ClusterNode): node is ClusterNodeStats {
  return "wal" in node && node.wal !== undefined;
}

export interface ClusterHealth {
  status: "ok" | "degraded" | "critical";
  /** Null when some node did not answer: ownership is then unknown. */
  unowned_shards: number | null;
  poisoned_nodes: string[];
  suspect_nodes: string[];
  partial?: boolean;
}

export interface ClusterOverview {
  cluster_id: string;
  this_node: string;
  clustered: boolean;
  num_shards: number;
  health: ClusterHealth;
  nodes: ClusterNode[];
}

/** One lease in the bucket, as this node sees it (`GET /api/v1/cluster/members`). */
export interface ClusterMember {
  node_id: string;
  inc: number;
  /** The lease's own state: alive, draining or dead. */
  state: string;
  status: NodeStatus;
  live: boolean;
  suspect: boolean;
  eligible: boolean;
  lease_age_ms: number;
  grpc_addr: string | null;
  http_addr: string | null;
  internal_addr: string | null;
  version: string;
  protocol: number;
  started_at: string | null;
  /** Earlier incarnations whose logs are not split yet. */
  unsplit: number[];
}

export interface ClusterPlacement {
  /** Operator pins: shard → node. */
  pins: Record<string, string>;
  effective_protocol: number;
  eligible_nodes: string[];
}

export type ShardPhase = "unowned" | "loading" | "serving" | "releasing";

export interface ShardStats {
  shard: number;
  owner: string | null;
  /** Ownership generation, bumped on every move (0 when not served). */
  gen: number;
  phase: ShardPhase | string;
  tasks: number;
  pending: number;
  running: number;
  retry: number;
  signals: number;
  dead_letters: number;
  shard_seq: number;
  snapshot_seq: number;
  /** Bucket key of the newest snapshot, ending in `{gen}-{seq}-{ptr}.snap`. */
  snapshot_key: string | null;
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

// ─── Admin ─────────────────────────────────────────────────────────

export interface QueuePartition {
  partition: number;
  shard: number;
  desired_owner: string | null;
  /** Removed by a shrink: no new tasks, served until empty. */
  draining: boolean;
}

export interface QueueInfo {
  queue: string;
  /** Partitions new tasks go to. */
  partitions: number;
  /** Every partition the queue ever had (removed ones drain). */
  served_partitions: number;
  previous: { partitions: number; changed_at: string } | null;
  version: number;
  created_at: string;
  partition_shards: QueuePartition[];
}

export interface AuditRecord {
  id: string;
  action: string;
  params: Record<string, unknown>;
  result: Record<string, unknown>;
  node: string;
  at: string;
}
