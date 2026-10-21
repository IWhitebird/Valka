import { randomUUID } from "node:crypto";
import { createChannel, createClient, Metadata, type Channel } from "nice-grpc";
import { TaskContext } from "./context.js";
import { ConnectionError, HandlerError } from "./errors.js";
import {
  CheckpointRequest,
  ResultStatus,
  WorkerServiceDefinition,
  type DeepPartial,
  type ResultAck,
  type TaskAssignment,
  type TaskResult,
  type WorkerRequest,
  type WorkerResponse,
  type RunningTask,
  type WorkerHello,
} from "./generated/valka/v1/worker.js";
import { RetryPolicy } from "./retry.js";
import type { TaskHandler, ValkaWorkerOptions } from "./types.js";

interface MessageChannel<T> {
  send(msg: T): void;
  close(): void;
  [Symbol.asyncIterator](): AsyncIterableIterator<T>;
}

function createMessageChannel<T>(): MessageChannel<T> {
  const queue: T[] = [];
  let resolve: (() => void) | null = null;
  let done = false;

  return {
    send(msg: T) {
      queue.push(msg);
      if (resolve) {
        resolve();
        resolve = null;
      }
    },
    close() {
      done = true;
      if (resolve) {
        resolve();
        resolve = null;
      }
    },
    [Symbol.asyncIterator](): AsyncIterableIterator<T> {
      const iter: AsyncIterableIterator<T> = {
        async next(): Promise<IteratorResult<T>> {
          while (queue.length === 0 && !done) {
            await new Promise<void>((r) => {
              resolve = r;
            });
          }
          if (queue.length > 0) {
            return { value: queue.shift()!, done: false };
          }
          return { value: undefined as unknown as T, done: true };
        },
        [Symbol.asyncIterator]() {
          return iter;
        },
      };
      return iter;
    },
  };
}

const RESULT_RETRY_DELAY_MS = 1_000;
/** A drain gives running handlers and their results this long from the shutdown request. */
const DRAIN_TIMEOUT_MS = 30_000;
const DRAIN_POLL_MS = 100;
/** Reconnect delay cap while draining: results are waiting for a connection. */
const DRAIN_RECONNECT_DELAY_MS = 1_000;

export class ValkaWorker {
  private readonly workerId: string;
  private readonly name: string;
  private readonly serverAddrs: string[];
  private nextAddr = 0;
  private readonly apiKey: string | undefined;
  private readonly queues: string[];
  private readonly concurrency: number;
  private readonly handler: TaskHandler;
  private readonly metadataStr: string;

  // Survive reconnects. Per-task state holds the latest run of each task: an older run of
  // the same task still finishing here is stale (the server starts a new run only after the
  // old one ended), and its cleanup never removes the newer run's state.
  //
  // task id -> run id of tasks running or awaiting a result ack (heartbeated on whichever
  // connection is current).
  private readonly running = new Map<string, string>();
  // Runs still executing, by task id: listed in every WorkerHello so the server can
  // reattach them to the new connection (or adopt or cancel them).
  private readonly runs = new Map<string, RunningTask>();
  private readonly unacked = new Map<string, TaskResult>();
  private readonly taskContexts = new Map<string, TaskContext>();
  private outbound: ((msg: DeepPartial<WorkerRequest>) => void) | null = null;
  private checkpointRpc: ((request: CheckpointRequest) => Promise<unknown>) | null = null;
  private activeCount = 0;
  private readonly slotWaiters: Array<() => void> = [];
  // Set once a graceful shutdown starts. The drain outlives connections: a session lost
  // meanwhile is re-established (on the next node) and told about the drain again.
  private drainDeadline: number | null = null;

  private constructor(options: ValkaWorkerOptions) {
    this.workerId = randomUUID();
    this.name = options.name ?? `worker-${randomUUID().slice(0, 8)}`;
    this.serverAddrs = options.serverAddrs?.length
      ? options.serverAddrs
      : [options.serverAddr ?? "localhost:50051"];
    this.apiKey =
      options.apiKey ??
      ((globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env
        ?.VALKA_API_KEY ||
        undefined);
    this.queues = options.queues;
    this.concurrency = options.concurrency ?? 1;
    this.handler = options.handler;
    this.metadataStr = options.metadata ? JSON.stringify(options.metadata) : "";
  }

  /**
   * Drain: the server sends no more tasks, running handlers finish and their results are
   * acknowledged (for up to 30 s), then `run()` resolves.
   */
  shutdown(): void {
    if (this.drainDeadline !== null) return;
    console.log("[valka] Shutdown requested, draining...");
    this.drainDeadline = Date.now() + DRAIN_TIMEOUT_MS;
    // The current session, if any; a later one sends its own after its hello.
    this.outbound?.({ shutdown: { reason: "shutdown" } });
  }

  /** Whether a drain is over: nothing runs or awaits an ack, or its time is up. */
  private drainFinished(): boolean {
    if (this.drainDeadline === null) return false;
    if (this.running.size === 0 && this.unacked.size === 0) return true;
    return Date.now() >= this.drainDeadline;
  }

  private acquireSlot(): Promise<void> {
    if (this.activeCount < this.concurrency) {
      this.activeCount++;
      return Promise.resolve();
    }
    return new Promise<void>((res) => this.slotWaiters.push(res));
  }

  private releaseSlot(): void {
    const next = this.slotWaiters.shift();
    if (next) {
      next();
    } else {
      this.activeCount--;
    }
  }

  /** Opens a session: lists runs still executing from before this connection. */
  helloMessage(): WorkerHello {
    return {
      workerId: this.workerId,
      workerName: this.name,
      queues: this.queues,
      concurrency: this.concurrency,
      metadata: this.metadataStr,
      running: Array.from(this.runs.values()),
    };
  }

  /** Keep the result until it is APPLIED or STALE; a later connection resends it. */
  private deliver(result: TaskResult): void {
    this.unacked.set(result.taskRunId, result);
    this.outbound?.({ taskResult: result });
  }

  private onResultAck(ack: ResultAck): void {
    const stale = ack.status === ResultStatus.RESULT_STATUS_STALE;
    if (stale || ack.status === ResultStatus.RESULT_STATUS_APPLIED) {
      if (stale) {
        console.warn(
          `[valka] Result for ${ack.taskId} not recorded: the run already ended another way`,
        );
      }
      this.unacked.delete(ack.taskRunId);
      // Only this run: a newer run of the same task may be running here.
      if (this.running.get(ack.taskId) === ack.taskRunId) this.running.delete(ack.taskId);
      return;
    }
    setTimeout(() => {
      const pending = this.unacked.get(ack.taskRunId);
      if (pending) this.outbound?.({ taskResult: pending });
    }, RESULT_RETRY_DELAY_MS);
  }

  static builder(): ValkaWorkerBuilder {
    return new ValkaWorkerBuilder();
  }

  static create(options: ValkaWorkerOptions): ValkaWorker {
    return new ValkaWorker(options);
  }

  /** Run until a graceful shutdown (SIGINT, SIGTERM or `shutdown()`) is over. */
  async run(): Promise<void> {
    const retryPolicy = new RetryPolicy();
    const proc = (globalThis as { process?: NodeJS.Process }).process;
    const onSignal = (): void => this.shutdown();
    proc?.once("SIGINT", onSignal);
    proc?.once("SIGTERM", onSignal);

    try {
      // eslint-disable-next-line no-constant-condition
      while (true) {
        let failure: unknown;
        try {
          await this.connectAndRun(retryPolicy);
          failure = new ConnectionError("the server closed the session");
        } catch (err) {
          failure = err;
        }
        if (this.drainFinished()) {
          console.log("[valka] Worker drained and disconnected");
          return;
        }
        // Next time, try the next node: any node takes any worker.
        this.nextAddr++;
        let delay = retryPolicy.nextDelay();
        if (this.drainDeadline !== null) delay = Math.min(delay, DRAIN_RECONNECT_DELAY_MS);
        console.warn(
          `[valka] Connection lost: ${failure instanceof Error ? failure.message : failure}. ` +
            `Reconnecting in ${delay}ms...`,
        );
        if (await this.waitToReconnect(delay)) {
          console.log("[valka] Worker drained while disconnected");
          return;
        }
      }
    } finally {
      proc?.removeListener("SIGINT", onSignal);
      proc?.removeListener("SIGTERM", onSignal);
    }
  }

  /** Sleep before reconnecting; true when a drain finished meanwhile. */
  private async waitToReconnect(delayMs: number): Promise<boolean> {
    let until = Date.now() + delayMs;
    while (Date.now() < until) {
      if (this.drainFinished()) return true;
      if (this.drainDeadline !== null) {
        until = Math.min(until, Date.now() + DRAIN_RECONNECT_DELAY_MS);
      }
      await sleep(Math.max(1, Math.min(DRAIN_POLL_MS, until - Date.now())));
    }
    return this.drainFinished();
  }

  /** One session. Resolves when the stream ends, rejects on errors and ServerShutdown. */
  private async connectAndRun(retryPolicy: RetryPolicy): Promise<void> {
    const channel: Channel = createChannel(
      this.serverAddrs[this.nextAddr % this.serverAddrs.length],
      undefined,
      // The server bounds what a task carries (its payload limit); gRPC's 4 MiB default
      // would break the stream on a large assignment.
      { "grpc.max_receive_message_length": -1 },
    );
    const client = createClient(
      WorkerServiceDefinition,
      channel,
      this.apiKey ? { "*": { metadata: Metadata({ authorization: `Bearer ${this.apiKey}` }) } } : {},
    );
    const requests = createMessageChannel<DeepPartial<WorkerRequest>>();
    const send = (msg: DeepPartial<WorkerRequest>): void => {
      requests.send(msg);
    };
    let heartbeatInterval: ReturnType<typeof setInterval> | undefined;
    let drainTimer: ReturnType<typeof setInterval> | undefined;
    let serverStopping = false;

    console.log(
      `[valka] Connecting: worker_id=${this.workerId} name=${this.name} ` +
        `queues=[${this.queues.join(",")}] concurrency=${this.concurrency}`,
    );

    try {
      send({ hello: this.helloMessage() });
      if (this.drainDeadline !== null) {
        // A drain carried over from an earlier session: no new tasks on this one either.
        send({ shutdown: { reason: "shutdown" } });
      }

      // Register this connection before resending: a result delivered concurrently is
      // then either in the resend below or sent on this connection.
      this.outbound = send;
      // ts-proto's Exact<> generic on fromPartial collapses nice-grpc's inferred request type
      // to an index signature of `never`; the runtime type is correct.
      this.checkpointRpc = (request) => client.checkpoint(request as never);
      for (const result of this.unacked.values()) {
        send({ taskResult: result });
      }

      heartbeatInterval = setInterval(() => {
        send({
          heartbeat: {
            activeTaskIds: Array.from(this.running.keys()),
            timestampMs: Date.now(),
          },
        });
      }, 10_000);
      // Once a drain is over, end the request stream: the server then closes the session.
      drainTimer = setInterval(() => {
        if (this.drainFinished()) requests.close();
      }, DRAIN_POLL_MS);

      let answered = false;
      // eslint-disable-next-line @typescript-eslint/no-explicit-any -- ts-proto's Exact
      // constraint + TS 5.7's IteratorResult change cause a deep structural mismatch
      // with AsyncIterable generics. The runtime types are correct.
      for await (const response of client.session(requests as any)) {
        if (!answered) {
          // The server took the session: back off from scratch after it ends.
          answered = true;
          retryPolicy.reset();
        }
        if (response.serverShutdown) {
          console.log(`[valka] Server shutting down: ${response.serverShutdown.reason}`);
          serverStopping = true;
          break;
        }
        try {
          this.handleResponse(response);
        } catch (err) {
          console.error("[valka] Could not handle a message from the server:", err);
        }
      }
    } finally {
      if (this.outbound === send) {
        this.outbound = null;
        this.checkpointRpc = null;
      }
      clearInterval(heartbeatInterval);
      clearInterval(drainTimer);
      requests.close();
      channel.close();
    }
    if (serverStopping) {
      // Reconnect to another node; a drain in progress goes on there.
      throw new ConnectionError("server shutting down");
    }
  }

  private handleResponse(response: WorkerResponse): void {
    if (response.taskAssignment) {
      this.startTask(response.taskAssignment);
    } else if (response.taskSignal) {
      const signal = response.taskSignal;
      this.taskContexts.get(signal.taskId)?._deliverSignal(signal);
    } else if (response.taskCancellation) {
      const cancel = response.taskCancellation;
      console.log(`[valka] Task cancelled: ${cancel.taskId} (${cancel.reason})`);
      this.running.delete(cancel.taskId);
      // Not listed on reconnect: the server would only cancel it again.
      this.runs.delete(cancel.taskId);
      this.taskContexts.delete(cancel.taskId);
    } else if (response.resultAck) {
      this.onResultAck(response.resultAck);
    }
  }

  private startTask(assignment: TaskAssignment): void {
    const { taskId, taskRunId } = assignment;
    // A newer run replaces an older one of the same task still finishing here: that one is
    // stale, and its result will be answered STALE.
    this.running.set(taskId, taskRunId);
    this.runs.set(taskId, { taskId, taskRunId, attemptNumber: assignment.attemptNumber });

    this.acquireSlot()
      .then(async () => {
        const ctx = new TaskContext(
          taskId,
          taskRunId,
          assignment.queueName,
          assignment.taskName,
          assignment.attemptNumber,
          assignment.input,
          assignment.metadata,
          (msg) => this.outbound?.(msg),
          assignment.checkpoints,
          async (step, output) => {
            const rpc = this.checkpointRpc;
            if (!rpc) {
              throw new ConnectionError("not connected to the server");
            }
            await rpc(CheckpointRequest.fromPartial({ taskId, taskRunId, step, output }));
          },
        );
        this.taskContexts.set(taskId, ctx);

        let result: TaskResult;
        try {
          const output = await this.handler(ctx);
          result = {
            taskId,
            taskRunId,
            success: true,
            retryable: false,
            output: output != null ? (JSON.stringify(output) ?? "") : "",
            errorMessage: "",
          };
        } catch (err) {
          result = {
            taskId,
            taskRunId,
            success: false,
            retryable: err instanceof HandlerError ? err.retryable : true,
            output: "",
            errorMessage: err instanceof Error ? err.message : String(err),
          };
        }
        // Only this run's state: a newer run of the same task may have replaced it.
        if (this.taskContexts.get(taskId) === ctx) this.taskContexts.delete(taskId);
        if (this.runs.get(taskId)?.taskRunId === taskRunId) this.runs.delete(taskId);
        this.releaseSlot();
        this.deliver(result);
      })
      .catch((err) => {
        console.error(`[valka] Task ${taskId} could not run:`, err);
      });
  }
}

// --- Builder ---

export class ValkaWorkerBuilder {
  private options: Partial<ValkaWorkerOptions> = {};

  name(name: string): this {
    this.options.name = name;
    return this;
  }

  serverAddr(addr: string): this {
    this.options.serverAddr = addr;
    return this;
  }

  /** Several nodes of one cluster; the worker moves to the next when its connection drops. */
  serverAddrs(addrs: string[]): this {
    this.options.serverAddrs = addrs;
    return this;
  }

  /** API key, sent as `authorization: Bearer <key>` (default: `VALKA_API_KEY`). */
  apiKey(key: string): this {
    this.options.apiKey = key;
    return this;
  }

  queues(queues: string[]): this {
    this.options.queues = queues;
    return this;
  }

  concurrency(n: number): this {
    this.options.concurrency = n;
    return this;
  }

  metadata(metadata: Record<string, unknown>): this {
    this.options.metadata = metadata;
    return this;
  }

  handler(fn: TaskHandler): this {
    this.options.handler = fn;
    return this;
  }

  build(): ValkaWorker {
    if (!this.options.queues || this.options.queues.length === 0) {
      throw new Error("At least one queue is required");
    }
    if (!this.options.handler) {
      throw new Error("A handler function is required");
    }
    return ValkaWorker.create(this.options as ValkaWorkerOptions);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
