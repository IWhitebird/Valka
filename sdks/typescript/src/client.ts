import { randomUUID } from "node:crypto";
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

/** Answers that may succeed on a retry, possibly on another node. */
const RETRYABLE_STATUS = new Set([429, 502, 503, 504]);

function envApiKey(): string | undefined {
  const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process
    ?.env;
  return env?.VALKA_API_KEY || undefined;
}

/**
 * REST client for the Valka API.
 *
 * Give it every node's URL (`baseUrls`): any node takes any request, so when one goes away
 * the client moves to the next. Creates carry an idempotency key (generated when you set
 * none), so a create retried after a lost response returns the task it already made. Reads
 * are retried the same way; cancels and signals are not.
 */
export class ValkaClient {
  private readonly baseUrls: string[];
  private next = 0;
  private readonly headers: Record<string, string>;
  private readonly attempts: number;
  private readonly timeoutMs: number;
  private readonly autoIdempotency: boolean;

  constructor(config: ValkaClientConfig | string) {
    const c: ValkaClientConfig = typeof config === "string" ? { baseUrl: config } : config;
    this.baseUrls = [c.baseUrl, ...(c.baseUrls ?? [])].map((u) => u.replace(/\/$/, ""));
    this.headers = { ...(c.headers ?? {}) };
    const apiKey = c.apiKey ?? envApiKey();
    if (apiKey) this.headers.Authorization = `Bearer ${apiKey}`;
    this.attempts = Math.max(1, c.attempts ?? 6);
    this.timeoutMs = c.timeoutMs ?? 10_000;
    this.autoIdempotency = c.autoIdempotency ?? true;
  }

  private get baseUrl(): string {
    return this.baseUrls[this.next % this.baseUrls.length];
  }

  // -- Task CRUD --

  /** Create a task. A second create with the same idempotency key returns the first task. */
  async createTask(options: CreateTaskOptions): Promise<Task> {
    const body = { ...options };
    if (!body.idempotency_key && this.autoIdempotency) {
      body.idempotency_key = `auto-${randomUUID()}`;
    }
    return this.request<Task>("POST", "/api/v1/tasks", body, Boolean(body.idempotency_key));
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

  /** Cancel a task (sent once: a cancel is not idempotent). */
  async cancelTask(taskId: string): Promise<Task> {
    return this.request<Task>("POST", `/api/v1/tasks/${taskId}/cancel`, undefined, false);
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

  // -- Signals --

  async sendSignal(
    taskId: string,
    signalName: string,
    payload?: unknown,
  ): Promise<{ signal_id: string; delivered: boolean }> {
    return this.request(
      "POST",
      `/api/v1/tasks/${taskId}/signal`,
      { signal_name: signalName, payload: payload ?? null },
      false,
    );
  }

  async listSignals(
    taskId: string,
    status?: string,
  ): Promise<unknown[]> {
    const params = new URLSearchParams();
    if (status) params.set("status", status);
    const qs = params.toString();
    return this.get(`/api/v1/tasks/${taskId}/signals${qs ? `?${qs}` : ""}`);
  }

  // -- Health --

  async healthCheck(): Promise<string> {
    const res = await this.send("GET", "/healthz", undefined, true);
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

  private async get<T>(path: string): Promise<T> {
    return this.request<T>("GET", path, undefined, true);
  }

  private async request<T>(
    method: string,
    path: string,
    body: unknown,
    retry: boolean,
  ): Promise<T> {
    const res = await this.send(method, path, body, retry);
    return res.json() as Promise<T>;
  }

  /** Send one request; a retryable failure moves to the next node and tries again. */
  private async send(
    method: string,
    path: string,
    body: unknown,
    retry: boolean,
  ): Promise<Response> {
    const attempts = retry ? this.attempts : 1;
    let delay = 100;
    let last: unknown = new Error("no attempt made");
    for (let attempt = 0; attempt < attempts; attempt++) {
      if (attempt > 0) {
        await new Promise((r) => setTimeout(r, delay));
        delay = Math.min(delay * 2, 3_000);
      }
      const node = this.next;
      let res: Response;
      try {
        res = await fetch(`${this.baseUrls[node % this.baseUrls.length]}${path}`, {
          method,
          headers: { "Content-Type": "application/json", ...this.headers },
          body: body === undefined ? undefined : JSON.stringify(body),
          signal: AbortSignal.timeout(this.timeoutMs),
        });
      } catch (err) {
        // Connection refused, reset, or no answer in time.
        last = err;
        this.advance(node);
        continue;
      }
      if (res.ok) return res;
      last = new ApiError(res.status, await this.extractErrorMessage(res));
      if (!RETRYABLE_STATUS.has(res.status)) throw last;
      this.advance(node);
    }
    throw last;
  }

  private advance(node: number): void {
    if (this.next === node) this.next = node + 1;
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
