export class ValkaError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ValkaError";
  }
}

export class ConnectionError extends ValkaError {
  constructor(message: string) {
    super(message);
    this.name = "ConnectionError";
  }
}

export class ApiError extends ValkaError {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export class HandlerError extends ValkaError {
  readonly retryable: boolean;

  constructor(message: string, retryable = true) {
    super(message);
    this.name = "HandlerError";
    this.retryable = retryable;
  }
}

export class NotConnectedError extends ValkaError {
  constructor() {
    super("Worker not connected");
    this.name = "NotConnectedError";
  }
}

export class ShuttingDownError extends ValkaError {
  constructor() {
    super("Shutdown in progress");
    this.name = "ShuttingDownError";
  }
}
