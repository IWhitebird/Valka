import { fetchAPI } from "./client";
import type {
  Task,
  TaskRun,
  TaskLog,
  CreateTaskRequest,
  ListTasksParams,
  DeadLetter,
  ListDeadLettersParams,
} from "./types";

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
    return fetchAPI<Task>(`/api/v1/tasks/${taskId}`);
  },

  create(request: CreateTaskRequest): Promise<Task> {
    return fetchAPI<Task>("/api/v1/tasks", {
      method: "POST",
      body: JSON.stringify(request),
    });
  },

  cancel(taskId: string): Promise<Task> {
    return fetchAPI<Task>(`/api/v1/tasks/${taskId}/cancel`, {
      method: "POST",
    });
  },

  getRuns(taskId: string): Promise<TaskRun[]> {
    return fetchAPI<TaskRun[]>(`/api/v1/tasks/${taskId}/runs`);
  },

  getRunLogs(taskId: string, runId: string): Promise<TaskLog[]> {
    return fetchAPI<TaskLog[]>(
      `/api/v1/tasks/${taskId}/runs/${runId}/logs`,
    );
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
