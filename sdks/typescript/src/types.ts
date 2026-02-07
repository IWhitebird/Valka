// --- Task Status (matches DB strings returned by REST API) ---
export type TaskStatus =
  | "PENDING"
  | "DISPATCHING"
  | "RUNNING"
  | "COMPLETED"
  | "FAILED"
  | "RETRY"
  | "DEAD_LETTER"
  | "CANCELLED";

// --- Log level (matches proto LogLevel enum) ---
export enum LogLevel {
  DEBUG = 1,
  INFO = 2,
  WARN = 3,
  ERROR = 4,
}

// --- REST API response types ---

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
  input: unknown | null;
  metadata: unknown | null;
  output: unknown | null;
  error_message: string | null;
  scheduled_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface TaskRun {
  id: string;
  task_id: string;
  attempt_number: number;
  worker_id: string | null;
  assigned_node_id: string;
  status: string;
  output: unknown | null;
  error_message: string | null;
  lease_expires_at: string;
  started_at: string;
  completed_at: string | null;
  last_heartbeat: string;
}

export interface TaskLog {
  id: number;
  task_run_id: string;
  timestamp_ms: number;
  level: number;
  message: string;
  metadata: unknown | null;
}

export interface WorkerInfo {
  id: string;
  name: string;
  queues: string[];
  concurrency: number;
  active_tasks: number;
  status: string;
  last_heartbeat: string;
  connected_at: string;
}

export interface DeadLetter {
  id: string;
  task_id: string;
  queue_name: string;
  task_name: string;
  input: unknown | null;
  error_message: string | null;
  attempt_count: number;
  metadata: unknown | null;
  created_at: string;
}

export interface TaskEvent {
  event_id: string;
  task_id: string;
  queue_name: string;
  new_status: number;
  timestamp_ms: number;
}

// --- Request types ---

export interface CreateTaskOptions {
  queue_name: string;
  task_name: string;
  input?: unknown;
  priority?: number;
  max_retries?: number;
  timeout_seconds?: number;
  idempotency_key?: string;
  metadata?: Record<string, unknown>;
  scheduled_at?: string;
}

export interface ListTasksOptions {
  queue_name?: string;
  status?: TaskStatus | string;
  limit?: number;
  offset?: number;
}

export interface ListDeadLettersOptions {
  queue_name?: string;
  limit?: number;
  offset?: number;
}

export interface GetRunLogsOptions {
  limit?: number;
  after_id?: number;
}

// --- Config types ---

export interface ValkaClientConfig {
  baseUrl: string;
  headers?: Record<string, string>;
}

export interface ValkaWorkerOptions {
  name?: string;
  serverAddr?: string;
  queues: string[];
  concurrency?: number;
  metadata?: Record<string, unknown>;
  handler: TaskHandler;
}

// --- Handler type ---

export type TaskHandler = (ctx: import("./context").TaskContext) => Promise<unknown>;
