import * as grpc from "@grpc/grpc-js";
import * as protoLoader from "@grpc/proto-loader";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Support both ESM (import.meta.url) and CJS (__dirname)
const currentDir =
  typeof __dirname !== "undefined"
    ? __dirname
    : path.dirname(fileURLToPath(import.meta.url));

// Proto files are at <package-root>/proto/valka/v1/
// dist/ is at <package-root>/dist/
const PROTO_DIR = path.resolve(currentDir, "..", "proto");

// --- Typed message interfaces matching proto definitions ---

export interface WorkerRequestMsg {
  hello?: WorkerHelloMsg;
  taskResult?: TaskResultMsg;
  heartbeat?: HeartbeatMsg;
  logBatch?: LogBatchMsg;
  shutdown?: GracefulShutdownMsg;
}

export interface WorkerResponseMsg {
  taskAssignment?: TaskAssignmentMsg;
  taskCancellation?: TaskCancellationMsg;
  heartbeatAck?: HeartbeatAckMsg;
  serverShutdown?: ServerShutdownMsg;
}

export interface WorkerHelloMsg {
  workerId: string;
  workerName: string;
  queues: string[];
  concurrency: number;
  metadata: string;
}

export interface TaskResultMsg {
  taskId: string;
  taskRunId: string;
  success: boolean;
  retryable: boolean;
  output: string;
  errorMessage: string;
}

export interface HeartbeatMsg {
  activeTaskIds: string[];
  timestampMs: number;
}

export interface LogBatchMsg {
  entries: LogEntryMsg[];
}

export interface LogEntryMsg {
  taskRunId: string;
  timestampMs: number;
  level: number;
  message: string;
  metadata: string;
}

export interface GracefulShutdownMsg {
  reason: string;
}

export interface TaskAssignmentMsg {
  taskId: string;
  taskRunId: string;
  queueName: string;
  taskName: string;
  input: string;
  attemptNumber: number;
  timeoutSeconds: number;
  metadata: string;
}

export interface TaskCancellationMsg {
  taskId: string;
  reason: string;
}

export interface HeartbeatAckMsg {
  serverTimestampMs: number;
}

export interface ServerShutdownMsg {
  reason: string;
  drainSeconds: number;
}

// --- gRPC client interface ---

export interface WorkerServiceClient {
  Session(): grpc.ClientDuplexStream<WorkerRequestMsg, WorkerResponseMsg>;
}

export function loadWorkerService(serverAddr: string): WorkerServiceClient {
  const workerProtoPath = path.join(PROTO_DIR, "valka", "v1", "worker.proto");

  const packageDefinition = protoLoader.loadSync(workerProtoPath, {
    keepCase: false,
    longs: Number,
    enums: Number,
    defaults: true,
    oneofs: true,
    includeDirs: [PROTO_DIR],
  });

  const protoDescriptor = grpc.loadPackageDefinition(packageDefinition);

  // Navigate to valka.v1.WorkerService
  const valka = protoDescriptor.valka as Record<string, unknown>;
  const v1 = valka.v1 as Record<string, unknown>;
  const WorkerServiceClass = v1.WorkerService as new (
    addr: string,
    creds: grpc.ChannelCredentials,
  ) => grpc.Client;

  const client = new WorkerServiceClass(
    serverAddr,
    grpc.credentials.createInsecure(),
  );

  return client as unknown as WorkerServiceClient;
}
