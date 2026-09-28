import { randomUUID } from "node:crypto";
import { createChannel, createClient, type Channel } from "nice-grpc";
import { TaskContext } from "./context.js";
import { ConnectionError, HandlerError } from "./errors.js";
import {
  CheckpointRequest,
  ResultStatus,
  WorkerServiceDefinition,
  type DeepPartial,
  type ResultAck,
  type TaskResult,
  type WorkerRequest,
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

export class ValkaWorker {
  private readonly workerId: string;
  private readonly name: string;
  private readonly serverAddr: string;
  private readonly queues: string[];
  private readonly concurrency: number;
  private readonly handler: TaskHandler;
  private readonly metadataStr: string;
  private shutdownResolve: (() => void) | null = null;
  private readonly shutdownPromise: Promise<void>;

  // Survive reconnects: tasks running or awaiting a result ack (heartbeated on whichever
  // connection is current), results not yet acknowledged, and the current connection.
  private readonly running = new Set<string>();
  private readonly unacked = new Map<string, TaskResult>();
  private readonly taskContexts = new Map<string, TaskContext>();
  private outbound: ((msg: DeepPartial<WorkerRequest>) => void) | null = null;
  private checkpointRpc: ((request: CheckpointRequest) => Promise<unknown>) | null = null;
  private activeCount = 0;
  private readonly slotWaiters: Array<() => void> = [];

  private constructor(options: ValkaWorkerOptions) {
    this.workerId = randomUUID();
    this.name = options.name ?? `worker-${randomUUID().slice(0, 8)}`;
    this.serverAddr = options.serverAddr ?? "localhost:50051";
    this.queues = options.queues;
    this.concurrency = options.concurrency ?? 1;
    this.handler = options.handler;
    this.metadataStr = options.metadata ? JSON.stringify(options.metadata) : "";
    this.shutdownPromise = new Promise((resolve) => {
      this.shutdownResolve = resolve;
    });
  }

  shutdown(): void {
    if (this.shutdownResolve) {
      this.shutdownResolve();
      this.shutdownResolve = null;
    }
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
      this.running.delete(ack.taskId);
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

  async run(): Promise<void> {
    const retryPolicy = new RetryPolicy();

    // eslint-disable-next-line no-constant-condition
    while (true) {
      try {
        await this.connectAndRun(retryPolicy);
        console.log("[valka] Worker disconnected gracefully");
        return;
      } catch (err) {
        const delay = retryPolicy.nextDelay();
        console.warn(
          `[valka] Connection lost: ${err instanceof Error ? err.message : err}. ` +
            `Reconnecting in ${delay}ms...`,
        );
        await sleep(delay);
      }
    }
  }

  private async connectAndRun(retryPolicy: RetryPolicy): Promise<void> {
    const channel: Channel = createChannel(this.serverAddr);
    const client = createClient(WorkerServiceDefinition, channel);
    const requests = createMessageChannel<DeepPartial<WorkerRequest>>();

    retryPolicy.reset();
    console.log(
      `[valka] Connected: worker_id=${this.workerId} name=${this.name} ` +
        `queues=[${this.queues.join(",")}] concurrency=${this.concurrency}`,
    );

    let gracefulShutdown = false;
    let serverStopping = false;

    const send = (msg: DeepPartial<WorkerRequest>): void => {
      requests.send(msg);
    };

    // Send WorkerHello
    send({
      hello: {
        workerId: this.workerId,
        workerName: this.name,
        queues: this.queues,
        concurrency: this.concurrency,
        metadata: this.metadataStr,
      },
    });

    // Register this connection before resending: a result delivered concurrently is
    // then either in the resend below or sent on this connection.
    this.outbound = send;
    // ts-proto's Exact<> generic on fromPartial collapses nice-grpc's inferred request type
    // to an index signature of `never`; the runtime type is correct.
    this.checkpointRpc = (request) => client.checkpoint(request as never);
    for (const result of this.unacked.values()) {
      send({ taskResult: result });
    }

    const heartbeatInterval = setInterval(() => {
      send({
        heartbeat: {
          activeTaskIds: Array.from(this.running),
          timestampMs: Date.now(),
        },
      });
    }, 10_000);

    // Graceful shutdown on signals
    const shutdownHandler = () => {
      if (gracefulShutdown) return;
      console.log("[valka] Shutdown signal received, draining...");
      gracefulShutdown = true;
      send({ shutdown: { reason: "SIGINT" } });

      const checkDrained = setInterval(() => {
        if (this.running.size === 0) {
          clearInterval(checkDrained);
          requests.close();
        }
      }, 100);

      // Safety timeout: force close after 30s
      setTimeout(() => {
        clearInterval(checkDrained);
        requests.close();
      }, 30_000);
    };

    process.once("SIGINT", shutdownHandler);
    process.once("SIGTERM", shutdownHandler);
    this.shutdownPromise.then(shutdownHandler);

    const cleanup = () => {
      this.outbound = null;
      this.checkpointRpc = null;
      clearInterval(heartbeatInterval);
      process.removeListener("SIGINT", shutdownHandler);
      process.removeListener("SIGTERM", shutdownHandler);
    };

    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any -- ts-proto's Exact
      // constraint + TS 5.7's IteratorResult change cause a deep structural mismatch
      // with AsyncIterable generics. The runtime types are correct.
      for await (const response of client.session(requests as any)) {
        if (response.taskAssignment) {
          const assignment = response.taskAssignment;
          this.running.add(assignment.taskId);

          this.acquireSlot().then(async () => {
            const ctx = new TaskContext(
              assignment.taskId,
              assignment.taskRunId,
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
                await rpc(
                  CheckpointRequest.fromPartial({
                    taskId: assignment.taskId,
                    taskRunId: assignment.taskRunId,
                    step,
                    output,
                  }),
                );
              },
            );
            this.taskContexts.set(assignment.taskId, ctx);

            let result: TaskResult;
            try {
              const output = await this.handler(ctx);
              result = {
                taskId: assignment.taskId,
                taskRunId: assignment.taskRunId,
                success: true,
                retryable: false,
                output: output != null ? JSON.stringify(output) : "",
                errorMessage: "",
              };
            } catch (err) {
              result = {
                taskId: assignment.taskId,
                taskRunId: assignment.taskRunId,
                success: false,
                retryable: err instanceof HandlerError ? err.retryable : true,
                output: "",
                errorMessage: err instanceof Error ? err.message : String(err),
              };
            }
            this.taskContexts.delete(assignment.taskId);
            this.releaseSlot();
            this.deliver(result);
          });
        } else if (response.taskSignal) {
          const signal = response.taskSignal;
          const ctx = this.taskContexts.get(signal.taskId);
          if (ctx) {
            ctx._deliverSignal(signal);
          }
        } else if (response.taskCancellation) {
          const cancel = response.taskCancellation;
          console.log(`[valka] Task cancelled: ${cancel.taskId} (${cancel.reason})`);
          this.running.delete(cancel.taskId);
          this.taskContexts.delete(cancel.taskId);
        } else if (response.resultAck) {
          this.onResultAck(response.resultAck);
        } else if (response.heartbeatAck) {
          // No-op
        } else if (response.serverShutdown) {
          const shutdown = response.serverShutdown;
          console.log(`[valka] Server shutting down: ${shutdown.reason}`);
          serverStopping = true;
          break;
        }
      }
    } catch (err) {
      cleanup();
      if (gracefulShutdown) return;
      throw new ConnectionError(err instanceof Error ? err.message : String(err));
    }

    cleanup();
    requests.close();
    channel.close();
    if (serverStopping && !gracefulShutdown) {
      throw new ConnectionError("server shutting down");
    }
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
