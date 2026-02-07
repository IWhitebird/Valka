import { ApiError } from "./errors.js";
import type {
  Task,
  TaskRun,
  TaskLog,
  WorkerInfo,
  DeadLetter,
  TaskEvent,
  CreateTaskOptions,
  ListTasksOptions,
  ListDeadLettersOptions,
  GetRunLogsOptions,
  ValkaClientConfig,
} from "./types.js";

export class ValkaClient {
  private baseUrl: string;
  private headers: Record<string, string>;

  constructor(config: ValkaClientConfig | string) {
    if (typeof config === "string") {
      this.baseUrl = config.replace(/\/$/, "");
      this.headers = {};
    } else {
      this.baseUrl = config.baseUrl.replace(/\/$/, "");
      this.headers = config.headers ?? {};
    }
  }

  // -- Task CRUD --

  async createTask(options: CreateTaskOptions): Promise<Task> {
    return this.post<Task>("/api/v1/tasks", options);
  }

  async getTask(taskId: string): Promise<Task> {
    return this.get<Task>(`/api/v1/tasks/${taskId}`);
  }

  async listTasks(options: ListTasksOptions = {}): Promise<Task[]> {
    const params = new URLSearchParams();
    if (options.queue_name) params.set("queue_name", options.queue_name);
    if (options.status) params.set("status", options.status);
    if (options.limit !== undefined) params.set("limit", String(options.limit));
    if (options.offset !== undefined) params.set("offset", String(options.offset));
    const qs = params.toString();
    return this.get<Task[]>(`/api/v1/tasks${qs ? `?${qs}` : ""}`);
  }

  async cancelTask(taskId: string): Promise<Task> {
    return this.post<Task>(`/api/v1/tasks/${taskId}/cancel`);
  }

  // -- Task Runs & Logs --

  async getTaskRuns(taskId: string): Promise<TaskRun[]> {
    return this.get<TaskRun[]>(`/api/v1/tasks/${taskId}/runs`);
  }

  async getRunLogs(
    taskId: string,
    runId: string,
    options: GetRunLogsOptions = {},
  ): Promise<TaskLog[]> {
    const params = new URLSearchParams();
    if (options.limit !== undefined) params.set("limit", String(options.limit));
    if (options.after_id !== undefined) params.set("after_id", String(options.after_id));
    const qs = params.toString();
    return this.get<TaskLog[]>(
      `/api/v1/tasks/${taskId}/runs/${runId}/logs${qs ? `?${qs}` : ""}`,
    );
  }

  // -- Workers --

  async listWorkers(): Promise<WorkerInfo[]> {
    return this.get<WorkerInfo[]>("/api/v1/workers");
  }

  // -- Dead Letters --

  async listDeadLetters(options: ListDeadLettersOptions = {}): Promise<DeadLetter[]> {
    const params = new URLSearchParams();
    if (options.queue_name) params.set("queue_name", options.queue_name);
    if (options.limit !== undefined) params.set("limit", String(options.limit));
    if (options.offset !== undefined) params.set("offset", String(options.offset));
    const qs = params.toString();
    return this.get<DeadLetter[]>(`/api/v1/dead-letters${qs ? `?${qs}` : ""}`);
  }

  // -- Health --

  async healthCheck(): Promise<string> {
    const res = await this.fetchRaw("/healthz");
    return res.text();
  }

  // -- SSE Events --

  subscribeEvents(
    onEvent: (event: TaskEvent) => void,
    onError?: (error: Error) => void,
  ): () => void {
    const controller = new AbortController();

    const connect = async () => {
      try {
        const response = await fetch(`${this.baseUrl}/api/v1/events`, {
          headers: { ...this.headers, Accept: "text/event-stream" },
          signal: controller.signal,
        });

        if (!response.ok || !response.body) {
          throw new ApiError(response.status, "Failed to connect to event stream");
        }

        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";

        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          const lines = buffer.split("\n");
          buffer = lines.pop() ?? "";
          for (const line of lines) {
            if (line.startsWith("data: ")) {
              try {
                const data = JSON.parse(line.slice(6)) as TaskEvent;
                onEvent(data);
              } catch {
                /* skip malformed events */
              }
            }
          }
        }
      } catch (err) {
        if (!controller.signal.aborted) {
          onError?.(err instanceof Error ? err : new Error(String(err)));
        }
      }
    };

    connect();
    return () => controller.abort();
  }

  // -- Internal helpers --

  private async fetchRaw(path: string, init: RequestInit = {}): Promise<Response> {
    return fetch(`${this.baseUrl}${path}`, {
      ...init,
      headers: {
        "Content-Type": "application/json",
        ...this.headers,
        ...(init.headers as Record<string, string> | undefined),
      },
    });
  }

  private async get<T>(path: string): Promise<T> {
    const res = await this.fetchRaw(path);
    if (!res.ok) {
      throw new ApiError(res.status, await this.extractErrorMessage(res));
    }
    return res.json() as Promise<T>;
  }

  private async post<T>(path: string, body?: unknown): Promise<T> {
    const res = await this.fetchRaw(path, {
      method: "POST",
      body: body ? JSON.stringify(body) : undefined,
    });
    if (!res.ok) {
      throw new ApiError(res.status, await this.extractErrorMessage(res));
    }
    return res.json() as Promise<T>;
  }

  private async extractErrorMessage(res: Response): Promise<string> {
    try {
      const text = await res.text();
      return text || `HTTP ${res.status}`;
    } catch {
      return `HTTP ${res.status}`;
    }
  }
}
