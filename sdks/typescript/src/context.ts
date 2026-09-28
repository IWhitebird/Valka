import type {
  DeepPartial,
  StepCheckpoint,
  WorkerRequest,
} from "./generated/valka/v1/worker.js";
import { LogLevel } from "./types.js";

type SendFn = (msg: DeepPartial<WorkerRequest>) => void;
type CheckpointFn = (step: string, output: string) => Promise<void>;

export interface SignalData {
  signalId: string;
  name: string;
  payload: string;
}

interface SignalWaiter {
  name: string | null; // null = any signal
  resolve: (data: SignalData) => void;
}

export class TaskContext {
  readonly taskId: string;
  readonly taskRunId: string;
  readonly queueName: string;
  readonly taskName: string;
  readonly attemptNumber: number;

  private readonly rawInput: string;
  private readonly rawMetadata: string;
  private readonly sendFn: SendFn;
  private readonly checkpointFn: CheckpointFn;
  private readonly checkpoints: Map<string, string>;
  private readonly signalBuffer: SignalData[] = [];
  private readonly signalWaiters: SignalWaiter[] = [];

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
    checkpoints: StepCheckpoint[],
    checkpointFn: CheckpointFn,
  ) {
    this.taskId = taskId;
    this.taskRunId = taskRunId;
    this.queueName = queueName;
    this.taskName = taskName;
    this.attemptNumber = attemptNumber;
    this.rawInput = input;
    this.rawMetadata = metadata;
    this.sendFn = sendFn;
    this.checkpoints = new Map(checkpoints.map((c) => [c.step, c.output]));
    this.checkpointFn = checkpointFn;
  }

  /**
   * Run `fn` as the step `name`, or return its result from an earlier attempt that already
   * checkpointed it. The result must be JSON-serializable and is checkpointed before this
   * resolves, so a retry of the task skips the step.
   */
  async step<T>(name: string, fn: () => T | Promise<T>): Promise<T> {
    if (this.checkpoints.has(name)) {
      return this.checkpointValue<T>(name) as T;
    }
    const value = await fn();
    await this.checkpoint(name, value);
    return value;
  }

  /** Durably record `value` as the result of `step` for this task. */
  async checkpoint(step: string, value: unknown): Promise<void> {
    const output = JSON.stringify(value ?? null);
    await this.checkpointFn(step, output);
    this.checkpoints.set(step, output);
  }

  /** The checkpointed result of `step`, or `undefined` if no attempt recorded one. */
  checkpointValue<T = unknown>(step: string): T | undefined {
    const output = this.checkpoints.get(step);
    return output === undefined ? undefined : (JSON.parse(output) as T);
  }

  /** Parse the task input JSON. */
  input<T = unknown>(): T {
    return JSON.parse(this.rawInput || "null") as T;
  }

  /** Parse the task metadata JSON. */
  metadata<T = Record<string, unknown>>(): T {
    return JSON.parse(this.rawMetadata || "{}") as T;
  }

  /** Wait for a signal with a specific name. Non-matching signals are buffered. */
  waitForSignal(name: string): Promise<SignalData> {
    // Check buffer first
    const idx = this.signalBuffer.findIndex((s) => s.name === name);
    if (idx >= 0) {
      const signal = this.signalBuffer.splice(idx, 1)[0];
      this.sendSignalAck(signal.signalId);
      return Promise.resolve(signal);
    }

    // Register waiter
    return new Promise<SignalData>((resolve) => {
      this.signalWaiters.push({ name, resolve });
    });
  }

  /** Wait for the next signal (any name). Checks buffer first. */
  receiveSignal(): Promise<SignalData> {
    if (this.signalBuffer.length > 0) {
      const signal = this.signalBuffer.shift()!;
      this.sendSignalAck(signal.signalId);
      return Promise.resolve(signal);
    }

    return new Promise<SignalData>((resolve) => {
      this.signalWaiters.push({ name: null, resolve });
    });
  }

  /** Parse a signal's JSON payload. */
  static parseSignalPayload<T = unknown>(signal: SignalData): T {
    return JSON.parse(signal.payload || "null") as T;
  }

  /** @internal — called by the worker to deliver signals to this context. */
  _deliverSignal(signal: {
    signalId: string;
    signalName: string;
    payload: string;
  }): void {
    const data: SignalData = {
      signalId: signal.signalId,
      name: signal.signalName,
      payload: signal.payload,
    };

    // Check if any waiter matches
    for (let i = 0; i < this.signalWaiters.length; i++) {
      const waiter = this.signalWaiters[i];
      if (waiter.name === null || waiter.name === data.name) {
        this.signalWaiters.splice(i, 1);
        this.sendSignalAck(data.signalId);
        waiter.resolve(data);
        return;
      }
    }

    // No waiter matched, buffer it
    this.signalBuffer.push(data);
  }

  private sendSignalAck(signalId: string): void {
    this.sendFn({ signalAck: { signalId } });
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
            level: level as number,
            message,
            metadata: "",
          },
        ],
      },
    });
  }
}
