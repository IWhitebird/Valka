import { randomUUID } from "node:crypto";
import { TaskContext } from "./context.js";
import { ConnectionError, HandlerError } from "./errors.js";
import {
  loadWorkerService,
  type WorkerRequestMsg,
  type WorkerResponseMsg,
} from "./proto-loader.js";
import { RetryPolicy } from "./retry.js";
import type { TaskHandler, ValkaWorkerOptions } from "./types.js";

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

  private connectAndRun(retryPolicy: RetryPolicy): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const grpcClient = loadWorkerService(this.serverAddr);
      const call = grpcClient.Session();

      retryPolicy.reset();
      console.log(
        `[valka] Connected: worker_id=${this.workerId} name=${this.name} ` +
          `queues=[${this.queues.join(",")}] concurrency=${this.concurrency}`,
      );

      let gracefulShutdown = false;
      const activeTasks = new Set<string>();
      let activeCount = 0;
      const pendingResolves: Array<() => void> = [];

      // Semaphore-like concurrency control
      const acquireSlot = (): Promise<void> => {
        if (activeCount < this.concurrency) {
          activeCount++;
          return Promise.resolve();
        }
        return new Promise<void>((res) => pendingResolves.push(res));
      };
      const releaseSlot = (): void => {
        if (pendingResolves.length > 0) {
          const next = pendingResolves.shift()!;
          next();
        } else {
          activeCount--;
        }
      };

      const send = (msg: WorkerRequestMsg): void => {
        call.write(msg);
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

      // Heartbeat every 10 seconds
      const heartbeatInterval = setInterval(() => {
        send({
          heartbeat: {
            activeTaskIds: Array.from(activeTasks),
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
          if (activeTasks.size === 0) {
            clearInterval(checkDrained);
            cleanup();
            call.end();
            resolve();
          }
        }, 100);

        // Safety timeout: force close after 30s
        setTimeout(() => {
          clearInterval(checkDrained);
          cleanup();
          call.end();
          resolve();
        }, 30_000);
      };

      process.once("SIGINT", shutdownHandler);
      process.once("SIGTERM", shutdownHandler);
      this.shutdownPromise.then(shutdownHandler);

      const cleanup = () => {
        clearInterval(heartbeatInterval);
        process.removeListener("SIGINT", shutdownHandler);
        process.removeListener("SIGTERM", shutdownHandler);
      };

      // Handle incoming messages from server
      call.on("data", (response: WorkerResponseMsg) => {
        if (response.taskAssignment) {
          const assignment = response.taskAssignment;
          activeTasks.add(assignment.taskId);

          acquireSlot().then(async () => {
            const ctx = new TaskContext(
              assignment.taskId,
              assignment.taskRunId,
              assignment.queueName,
              assignment.taskName,
              assignment.attemptNumber,
              assignment.input,
              assignment.metadata,
              send,
            );

            try {
              const output = await this.handler(ctx);
              send({
                taskResult: {
                  taskId: assignment.taskId,
                  taskRunId: assignment.taskRunId,
                  success: true,
                  retryable: false,
                  output: output != null ? JSON.stringify(output) : "",
                  errorMessage: "",
                },
              });
            } catch (err) {
              const isRetryable = err instanceof HandlerError ? err.retryable : true;
              const errorMessage = err instanceof Error ? err.message : String(err);
              send({
                taskResult: {
                  taskId: assignment.taskId,
                  taskRunId: assignment.taskRunId,
                  success: false,
                  retryable: isRetryable,
                  output: "",
                  errorMessage,
                },
              });
            } finally {
              activeTasks.delete(assignment.taskId);
              releaseSlot();
            }
          });
        } else if (response.taskCancellation) {
          const cancel = response.taskCancellation;
          console.log(`[valka] Task cancelled: ${cancel.taskId} (${cancel.reason})`);
          activeTasks.delete(cancel.taskId);
        } else if (response.heartbeatAck) {
          // No-op
        } else if (response.serverShutdown) {
          const shutdown = response.serverShutdown;
          console.log(`[valka] Server shutting down: ${shutdown.reason}`);
          cleanup();
          call.end();
          resolve();
        }
      });

      call.on("error", (err: Error) => {
        cleanup();
        if (gracefulShutdown) {
          resolve();
        } else {
          reject(new ConnectionError(err.message));
        }
      });

      call.on("end", () => {
        cleanup();
        if (gracefulShutdown) {
          resolve();
        } else {
          reject(new ConnectionError("Stream closed by server"));
        }
      });
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
