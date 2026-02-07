import type { WorkerRequestMsg } from "./proto-loader.js";
import { LogLevel } from "./types.js";

type SendFn = (msg: WorkerRequestMsg) => void;

export class TaskContext {
  readonly taskId: string;
  readonly taskRunId: string;
  readonly queueName: string;
  readonly taskName: string;
  readonly attemptNumber: number;

  private readonly rawInput: string;
  private readonly rawMetadata: string;
  private readonly sendFn: SendFn;

  /** @internal */
  constructor(
    taskId: string,
    taskRunId: string,
    queueName: string,
    taskName: string,
    attemptNumber: number,
    input: string,
    metadata: string,
    sendFn: SendFn,
  ) {
    this.taskId = taskId;
    this.taskRunId = taskRunId;
    this.queueName = queueName;
    this.taskName = taskName;
    this.attemptNumber = attemptNumber;
    this.rawInput = input;
    this.rawMetadata = metadata;
    this.sendFn = sendFn;
  }

  /** Parse the task input JSON. */
  input<T = unknown>(): T {
    return JSON.parse(this.rawInput || "null") as T;
  }

  /** Parse the task metadata JSON. */
  metadata<T = Record<string, unknown>>(): T {
    return JSON.parse(this.rawMetadata || "{}") as T;
  }

  /** Log at INFO level. */
  log(message: string): void {
    this.logAtLevel(LogLevel.INFO, message);
  }

  /** Log at DEBUG level. */
  debug(message: string): void {
    this.logAtLevel(LogLevel.DEBUG, message);
  }

  /** Log at WARN level. */
  warn(message: string): void {
    this.logAtLevel(LogLevel.WARN, message);
  }

  /** Log at ERROR level. */
  error(message: string): void {
    this.logAtLevel(LogLevel.ERROR, message);
  }

  private logAtLevel(level: LogLevel, message: string): void {
    this.sendFn({
      logBatch: {
        entries: [
          {
            taskRunId: this.taskRunId,
            timestampMs: Date.now(),
            level,
            message,
            metadata: "",
          },
        ],
      },
    });
  }
}
