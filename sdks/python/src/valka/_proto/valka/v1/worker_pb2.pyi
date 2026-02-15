from valka.v1 import common_pb2 as _common_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class WorkerRequest(_message.Message):
    __slots__ = ("hello", "task_result", "heartbeat", "log_batch", "shutdown", "signal_ack")
    HELLO_FIELD_NUMBER: _ClassVar[int]
    TASK_RESULT_FIELD_NUMBER: _ClassVar[int]
    HEARTBEAT_FIELD_NUMBER: _ClassVar[int]
    LOG_BATCH_FIELD_NUMBER: _ClassVar[int]
    SHUTDOWN_FIELD_NUMBER: _ClassVar[int]
    SIGNAL_ACK_FIELD_NUMBER: _ClassVar[int]
    hello: WorkerHello
    task_result: TaskResult
    heartbeat: Heartbeat
    log_batch: LogBatch
    shutdown: GracefulShutdown
    signal_ack: SignalAck
    def __init__(self, hello: _Optional[_Union[WorkerHello, _Mapping]] = ..., task_result: _Optional[_Union[TaskResult, _Mapping]] = ..., heartbeat: _Optional[_Union[Heartbeat, _Mapping]] = ..., log_batch: _Optional[_Union[LogBatch, _Mapping]] = ..., shutdown: _Optional[_Union[GracefulShutdown, _Mapping]] = ..., signal_ack: _Optional[_Union[SignalAck, _Mapping]] = ...) -> None: ...

class WorkerResponse(_message.Message):
    __slots__ = ("task_assignment", "task_cancellation", "heartbeat_ack", "server_shutdown", "task_signal")
    TASK_ASSIGNMENT_FIELD_NUMBER: _ClassVar[int]
    TASK_CANCELLATION_FIELD_NUMBER: _ClassVar[int]
    HEARTBEAT_ACK_FIELD_NUMBER: _ClassVar[int]
    SERVER_SHUTDOWN_FIELD_NUMBER: _ClassVar[int]
    TASK_SIGNAL_FIELD_NUMBER: _ClassVar[int]
    task_assignment: TaskAssignment
    task_cancellation: TaskCancellation
    heartbeat_ack: HeartbeatAck
    server_shutdown: ServerShutdown
    task_signal: TaskSignal
    def __init__(self, task_assignment: _Optional[_Union[TaskAssignment, _Mapping]] = ..., task_cancellation: _Optional[_Union[TaskCancellation, _Mapping]] = ..., heartbeat_ack: _Optional[_Union[HeartbeatAck, _Mapping]] = ..., server_shutdown: _Optional[_Union[ServerShutdown, _Mapping]] = ..., task_signal: _Optional[_Union[TaskSignal, _Mapping]] = ...) -> None: ...

class WorkerHello(_message.Message):
    __slots__ = ("worker_id", "worker_name", "queues", "concurrency", "metadata")
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    WORKER_NAME_FIELD_NUMBER: _ClassVar[int]
    QUEUES_FIELD_NUMBER: _ClassVar[int]
    CONCURRENCY_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    worker_id: str
    worker_name: str
    queues: _containers.RepeatedScalarFieldContainer[str]
    concurrency: int
    metadata: str
    def __init__(self, worker_id: _Optional[str] = ..., worker_name: _Optional[str] = ..., queues: _Optional[_Iterable[str]] = ..., concurrency: _Optional[int] = ..., metadata: _Optional[str] = ...) -> None: ...

class TaskResult(_message.Message):
    __slots__ = ("task_id", "task_run_id", "success", "retryable", "output", "error_message")
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    TASK_RUN_ID_FIELD_NUMBER: _ClassVar[int]
    SUCCESS_FIELD_NUMBER: _ClassVar[int]
    RETRYABLE_FIELD_NUMBER: _ClassVar[int]
    OUTPUT_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    task_id: str
    task_run_id: str
    success: bool
    retryable: bool
    output: str
    error_message: str
    def __init__(self, task_id: _Optional[str] = ..., task_run_id: _Optional[str] = ..., success: bool = ..., retryable: bool = ..., output: _Optional[str] = ..., error_message: _Optional[str] = ...) -> None: ...

class Heartbeat(_message.Message):
    __slots__ = ("active_task_ids", "timestamp_ms")
    ACTIVE_TASK_IDS_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    active_task_ids: _containers.RepeatedScalarFieldContainer[str]
    timestamp_ms: int
    def __init__(self, active_task_ids: _Optional[_Iterable[str]] = ..., timestamp_ms: _Optional[int] = ...) -> None: ...

class LogBatch(_message.Message):
    __slots__ = ("entries",)
    ENTRIES_FIELD_NUMBER: _ClassVar[int]
    entries: _containers.RepeatedCompositeFieldContainer[LogEntry]
    def __init__(self, entries: _Optional[_Iterable[_Union[LogEntry, _Mapping]]] = ...) -> None: ...

class LogEntry(_message.Message):
    __slots__ = ("task_run_id", "timestamp_ms", "level", "message", "metadata")
    TASK_RUN_ID_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    LEVEL_FIELD_NUMBER: _ClassVar[int]
    MESSAGE_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    task_run_id: str
    timestamp_ms: int
    level: _common_pb2.LogLevel
    message: str
    metadata: str
    def __init__(self, task_run_id: _Optional[str] = ..., timestamp_ms: _Optional[int] = ..., level: _Optional[_Union[_common_pb2.LogLevel, str]] = ..., message: _Optional[str] = ..., metadata: _Optional[str] = ...) -> None: ...

class GracefulShutdown(_message.Message):
    __slots__ = ("reason",)
    REASON_FIELD_NUMBER: _ClassVar[int]
    reason: str
    def __init__(self, reason: _Optional[str] = ...) -> None: ...

class TaskAssignment(_message.Message):
    __slots__ = ("task_id", "task_run_id", "queue_name", "task_name", "input", "attempt_number", "timeout_seconds", "metadata")
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    TASK_RUN_ID_FIELD_NUMBER: _ClassVar[int]
    QUEUE_NAME_FIELD_NUMBER: _ClassVar[int]
    TASK_NAME_FIELD_NUMBER: _ClassVar[int]
    INPUT_FIELD_NUMBER: _ClassVar[int]
    ATTEMPT_NUMBER_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_SECONDS_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    task_id: str
    task_run_id: str
    queue_name: str
    task_name: str
    input: str
    attempt_number: int
    timeout_seconds: int
    metadata: str
    def __init__(self, task_id: _Optional[str] = ..., task_run_id: _Optional[str] = ..., queue_name: _Optional[str] = ..., task_name: _Optional[str] = ..., input: _Optional[str] = ..., attempt_number: _Optional[int] = ..., timeout_seconds: _Optional[int] = ..., metadata: _Optional[str] = ...) -> None: ...

class TaskCancellation(_message.Message):
    __slots__ = ("task_id", "reason")
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    REASON_FIELD_NUMBER: _ClassVar[int]
    task_id: str
    reason: str
    def __init__(self, task_id: _Optional[str] = ..., reason: _Optional[str] = ...) -> None: ...

class HeartbeatAck(_message.Message):
    __slots__ = ("server_timestamp_ms",)
    SERVER_TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    server_timestamp_ms: int
    def __init__(self, server_timestamp_ms: _Optional[int] = ...) -> None: ...

class ServerShutdown(_message.Message):
    __slots__ = ("reason", "drain_seconds")
    REASON_FIELD_NUMBER: _ClassVar[int]
    DRAIN_SECONDS_FIELD_NUMBER: _ClassVar[int]
    reason: str
    drain_seconds: int
    def __init__(self, reason: _Optional[str] = ..., drain_seconds: _Optional[int] = ...) -> None: ...

class TaskSignal(_message.Message):
    __slots__ = ("signal_id", "task_id", "signal_name", "payload", "timestamp_ms")
    SIGNAL_ID_FIELD_NUMBER: _ClassVar[int]
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    SIGNAL_NAME_FIELD_NUMBER: _ClassVar[int]
    PAYLOAD_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    signal_id: str
    task_id: str
    signal_name: str
    payload: str
    timestamp_ms: int
    def __init__(self, signal_id: _Optional[str] = ..., task_id: _Optional[str] = ..., signal_name: _Optional[str] = ..., payload: _Optional[str] = ..., timestamp_ms: _Optional[int] = ...) -> None: ...

class SignalAck(_message.Message):
    __slots__ = ("signal_id",)
    SIGNAL_ID_FIELD_NUMBER: _ClassVar[int]
    signal_id: str
    def __init__(self, signal_id: _Optional[str] = ...) -> None: ...
