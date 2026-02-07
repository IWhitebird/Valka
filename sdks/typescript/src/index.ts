// Core classes
export { ValkaClient } from "./client.js";
export { ValkaWorker, ValkaWorkerBuilder } from "./worker.js";
export { TaskContext } from "./context.js";

// Errors
export {
  ValkaError,
  ApiError,
  ConnectionError,
  HandlerError,
  NotConnectedError,
  ShuttingDownError,
} from "./errors.js";

// Utilities
export { RetryPolicy } from "./retry.js";

// Types
export type {
  Task,
  TaskRun,
  TaskLog,
  WorkerInfo,
  DeadLetter,
  TaskEvent,
  TaskStatus,
  CreateTaskOptions,
  ListTasksOptions,
  ListDeadLettersOptions,
  GetRunLogsOptions,
  ValkaClientConfig,
  ValkaWorkerOptions,
  TaskHandler,
} from "./types.js";

export { LogLevel } from "./types.js";
