import { fetchAPI, fetchAPIResult, pathSegment } from "./client";
import type {
  Task,
  TaskRun,
  TaskCheckpoint,
  TaskLog,
  TaskSignal,
  CreateTaskRequest,
  SendSignalRequest,
  SendSignalResponse,
  ListTasksParams,
  DeadLetter,
  ListDeadLettersParams,
} from "./types";

/** `/api/v1/tasks/{id}` plus an optional suffix, with the id encoded. */
function taskPath(taskId: string, suffix = ""): string {
  return `/api/v1/tasks/${pathSegment(taskId)}${suffix}`;
}

export interface RunLogsParams {
  /** Only lines after this id (ids are 1-based positions in the run's log). */
  afterId?: number;
  /** At most this many lines (the server allows 1–10000, default 1000). */
  limit?: number;
}

export const tasksApi = {
  list(params: ListTasksParams = {}): Promise<Task[]> {
    const searchParams = new URLSearchParams();
    if (params.queue_name) searchParams.set("queue_name", params.queue_name);
    if (params.status) searchParams.set("status", params.status);
    if (params.limit !== undefined)
      searchParams.set("limit", String(params.limit));
    if (params.offset !== undefined)
      searchParams.set("offset", String(params.offset));

    const query = searchParams.toString();
    return fetchAPI<Task[]>(`/api/v1/tasks${query ? `?${query}` : ""}`);
  },

  get(taskId: string): Promise<Task> {
    return fetchAPI<Task>(taskPath(taskId));
  },

  create(request: CreateTaskRequest): Promise<Task> {
    return fetchAPI<Task>("/api/v1/tasks", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },

  cancel(taskId: string): Promise<Task> {
    return fetchAPI<Task>(taskPath(taskId, "/cancel"), {
      method: "POST",
    });
  },

  delete(taskId: string): Promise<{ deleted: boolean }> {
    return fetchAPI<{ deleted: boolean }>(taskPath(taskId), {
      method: "DELETE",
    });
  },

  /** Deletes every task on every node; `missing` names nodes that did not answer. */
  async clearAll(): Promise<{ deleted_count: number; missing: string[] | null }> {
    const { data, missing } = await fetchAPIResult<{ deleted_count: number }>("/api/v1/tasks", {
      method: "DELETE",
    });
    return { deleted_count: data.deleted_count, missing };
  },

  getRuns(taskId: string): Promise<TaskRun[]> {
    return fetchAPI<TaskRun[]>(taskPath(taskId, "/runs"));
  },

  getCheckpoints(taskId: string): Promise<TaskCheckpoint[]> {
    return fetchAPI<TaskCheckpoint[]>(taskPath(taskId, "/checkpoints"));
  },

  getRunLogs(taskId: string, runId: string, params: RunLogsParams = {}): Promise<TaskLog[]> {
    const searchParams = new URLSearchParams();
    if (params.afterId !== undefined) searchParams.set("after_id", String(params.afterId));
    if (params.limit !== undefined) searchParams.set("limit", String(params.limit));
    const query = searchParams.toString();
    return fetchAPI<TaskLog[]>(
      taskPath(taskId, `/runs/${pathSegment(runId)}/logs${query ? `?${query}` : ""}`),
    );
  },

  sendSignal(taskId: string, request: SendSignalRequest): Promise<SendSignalResponse> {
    return fetchAPI<SendSignalResponse>(taskPath(taskId, "/signal"), {
      method: "POST",
      body: JSON.stringify(request),
    });
  },

  listSignals(taskId: string, status?: string): Promise<TaskSignal[]> {
    const params = status ? `?status=${encodeURIComponent(status)}` : "";
    return fetchAPI<TaskSignal[]>(taskPath(taskId, `/signals${params}`));
  },

  listDeadLetters(params: ListDeadLettersParams = {}): Promise<DeadLetter[]> {
    const searchParams = new URLSearchParams();
    if (params.queue_name) searchParams.set("queue_name", params.queue_name);
    if (params.limit !== undefined)
      searchParams.set("limit", String(params.limit));
    if (params.offset !== undefined)
      searchParams.set("offset", String(params.offset));

    const query = searchParams.toString();
    return fetchAPI<DeadLetter[]>(
      `/api/v1/dead-letters${query ? `?${query}` : ""}`,
    );
  },
};
