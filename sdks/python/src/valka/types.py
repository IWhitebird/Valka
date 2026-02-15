"""Valka SDK type definitions."""

from __future__ import annotations

from enum import IntEnum
from typing import Any, TypedDict


class TaskStatus(IntEnum):
    """Task lifecycle status (matches proto enum values)."""

    UNSPECIFIED = 0
    PENDING = 1
    DISPATCHING = 2
    RUNNING = 3
    COMPLETED = 4
    FAILED = 5
    RETRY = 6
    DEAD_LETTER = 7
    CANCELLED = 8


class LogLevel(IntEnum):
    """Log severity level (matches proto enum values)."""

    DEBUG = 1
    INFO = 2
    WARN = 3
    ERROR = 4


class Task(TypedDict, total=False):
    """Task object returned by the REST API."""

    id: str
    queue_name: str
    task_name: str
    status: str
    priority: int
    max_retries: int
    attempt_count: int
    timeout_seconds: int
    idempotency_key: str | None
    input: Any
    metadata: Any
    output: Any
    error_message: str | None
    scheduled_at: str | None
    created_at: str
    updated_at: str


class TaskRun(TypedDict, total=False):
    """Task execution attempt."""

    id: str
    task_id: str
    attempt_number: int
    worker_id: str | None
    assigned_node_id: str | None
    status: str
    output: Any
    error_message: str | None
    lease_expires_at: str | None
    started_at: str | None
    completed_at: str | None
    last_heartbeat: str | None


class TaskLog(TypedDict, total=False):
    """Log entry for a task run."""

    id: str
    task_run_id: str
    timestamp_ms: int
    level: str
    message: str
    metadata: Any


class WorkerInfo(TypedDict, total=False):
    """Connected worker info."""

    id: str
    name: str
    queues: list[str]
    concurrency: int
    active_tasks: int
    status: str
    last_heartbeat: str
    connected_at: str


class DeadLetter(TypedDict, total=False):
    """Dead-lettered task entry."""

    id: str
    task_id: str
    queue_name: str
    task_name: str
    input: Any
    error_message: str | None
    attempt_count: int
    metadata: Any
    created_at: str


class TaskEvent(TypedDict, total=False):
    """Real-time task event from SSE."""

    event_id: str
    task_id: str
    queue_name: str
    new_status: int
    timestamp_ms: int


class Signal(TypedDict, total=False):
    """Task signal from the REST API."""

    id: str
    task_id: str
    signal_name: str
    payload: Any
    status: str
    created_at: str
    delivered_at: str | None
    acknowledged_at: str | None


class CreateTaskOptions(TypedDict, total=False):
    """Options for creating a task."""

    queue_name: str
    task_name: str
    input: Any
    priority: int
    max_retries: int
    timeout_seconds: int
    idempotency_key: str
    metadata: Any
    scheduled_at: str


class ListTasksOptions(TypedDict, total=False):
    """Options for listing tasks."""

    queue_name: str
    status: str
    limit: int
    offset: int


class ListDeadLettersOptions(TypedDict, total=False):
    """Options for listing dead-letter entries."""

    queue_name: str
    limit: int
    offset: int


class GetRunLogsOptions(TypedDict, total=False):
    """Options for getting run logs."""

    limit: int
    after_id: str
