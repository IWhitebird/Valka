"""Valka Python SDK — polyglot distributed task queue."""

from valka.client import ValkaClient
from valka.context import TaskContext
from valka.errors import (
    ApiError,
    ConnectionError,
    HandlerError,
    NotConnectedError,
    ShuttingDownError,
    ValkaError,
)
from valka.retry import RetryPolicy
from valka.types import (
    CreateTaskOptions,
    DeadLetter,
    GetRunLogsOptions,
    ListDeadLettersOptions,
    ListTasksOptions,
    LogLevel,
    Task,
    TaskEvent,
    TaskLog,
    TaskRun,
    TaskStatus,
    WorkerInfo,
)
from valka.worker import ValkaWorker, ValkaWorkerBuilder

__all__ = [
    # Core classes
    "ValkaClient",
    "ValkaWorker",
    "ValkaWorkerBuilder",
    "TaskContext",
    # Errors
    "ValkaError",
    "ApiError",
    "ConnectionError",
    "HandlerError",
    "NotConnectedError",
    "ShuttingDownError",
    # Utilities
    "RetryPolicy",
    # Types
    "Task",
    "TaskRun",
    "TaskLog",
    "TaskStatus",
    "LogLevel",
    "TaskEvent",
    "WorkerInfo",
    "DeadLetter",
    "CreateTaskOptions",
    "ListTasksOptions",
    "ListDeadLettersOptions",
    "GetRunLogsOptions",
]
