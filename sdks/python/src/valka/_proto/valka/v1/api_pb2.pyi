from valka.v1 import common_pb2 as _common_pb2
from valka.v1 import events_pb2 as _events_pb2
from valka.v1 import worker_pb2 as _worker_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Iterable as _Iterable, Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class CreateTaskRequest(_message.Message):
    __slots__ = ("queue_name", "task_name", "input", "priority", "max_retries", "timeout_seconds", "idempotency_key", "metadata", "scheduled_at")
    QUEUE_NAME_FIELD_NUMBER: _ClassVar[int]
    TASK_NAME_FIELD_NUMBER: _ClassVar[int]
    INPUT_FIELD_NUMBER: _ClassVar[int]
    PRIORITY_FIELD_NUMBER: _ClassVar[int]
    MAX_RETRIES_FIELD_NUMBER: _ClassVar[int]
    TIMEOUT_SECONDS_FIELD_NUMBER: _ClassVar[int]
    IDEMPOTENCY_KEY_FIELD_NUMBER: _ClassVar[int]
    METADATA_FIELD_NUMBER: _ClassVar[int]
    SCHEDULED_AT_FIELD_NUMBER: _ClassVar[int]
    queue_name: str
    task_name: str
    input: str
    priority: int
    max_retries: int
    timeout_seconds: int
    idempotency_key: str
    metadata: str
    scheduled_at: str
    def __init__(self, queue_name: _Optional[str] = ..., task_name: _Optional[str] = ..., input: _Optional[str] = ..., priority: _Optional[int] = ..., max_retries: _Optional[int] = ..., timeout_seconds: _Optional[int] = ..., idempotency_key: _Optional[str] = ..., metadata: _Optional[str] = ..., scheduled_at: _Optional[str] = ...) -> None: ...

class CreateTaskResponse(_message.Message):
    __slots__ = ("task",)
    TASK_FIELD_NUMBER: _ClassVar[int]
    task: _common_pb2.TaskMeta
    def __init__(self, task: _Optional[_Union[_common_pb2.TaskMeta, _Mapping]] = ...) -> None: ...

class GetTaskRequest(_message.Message):
    __slots__ = ("task_id",)
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    task_id: str
    def __init__(self, task_id: _Optional[str] = ...) -> None: ...

class GetTaskResponse(_message.Message):
    __slots__ = ("task",)
    TASK_FIELD_NUMBER: _ClassVar[int]
    task: _common_pb2.TaskMeta
    def __init__(self, task: _Optional[_Union[_common_pb2.TaskMeta, _Mapping]] = ...) -> None: ...

class ListTasksRequest(_message.Message):
    __slots__ = ("queue_name", "status", "pagination")
    QUEUE_NAME_FIELD_NUMBER: _ClassVar[int]
    STATUS_FIELD_NUMBER: _ClassVar[int]
    PAGINATION_FIELD_NUMBER: _ClassVar[int]
    queue_name: str
    status: _common_pb2.TaskStatus
    pagination: _common_pb2.Pagination
    def __init__(self, queue_name: _Optional[str] = ..., status: _Optional[_Union[_common_pb2.TaskStatus, str]] = ..., pagination: _Optional[_Union[_common_pb2.Pagination, _Mapping]] = ...) -> None: ...

class ListTasksResponse(_message.Message):
    __slots__ = ("tasks", "next_page_token")
    TASKS_FIELD_NUMBER: _ClassVar[int]
    NEXT_PAGE_TOKEN_FIELD_NUMBER: _ClassVar[int]
    tasks: _containers.RepeatedCompositeFieldContainer[_common_pb2.TaskMeta]
    next_page_token: str
    def __init__(self, tasks: _Optional[_Iterable[_Union[_common_pb2.TaskMeta, _Mapping]]] = ..., next_page_token: _Optional[str] = ...) -> None: ...

class CancelTaskRequest(_message.Message):
    __slots__ = ("task_id",)
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    task_id: str
    def __init__(self, task_id: _Optional[str] = ...) -> None: ...

class CancelTaskResponse(_message.Message):
    __slots__ = ("task",)
    TASK_FIELD_NUMBER: _ClassVar[int]
    task: _common_pb2.TaskMeta
    def __init__(self, task: _Optional[_Union[_common_pb2.TaskMeta, _Mapping]] = ...) -> None: ...

class SendSignalRequest(_message.Message):
    __slots__ = ("task_id", "signal_name", "payload")
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    SIGNAL_NAME_FIELD_NUMBER: _ClassVar[int]
    PAYLOAD_FIELD_NUMBER: _ClassVar[int]
    task_id: str
    signal_name: str
    payload: str
    def __init__(self, task_id: _Optional[str] = ..., signal_name: _Optional[str] = ..., payload: _Optional[str] = ...) -> None: ...

class SendSignalResponse(_message.Message):
    __slots__ = ("signal_id", "delivered")
    SIGNAL_ID_FIELD_NUMBER: _ClassVar[int]
    DELIVERED_FIELD_NUMBER: _ClassVar[int]
    signal_id: str
    delivered: bool
    def __init__(self, signal_id: _Optional[str] = ..., delivered: bool = ...) -> None: ...

class SubscribeLogsRequest(_message.Message):
    __slots__ = ("task_run_id", "include_history")
    TASK_RUN_ID_FIELD_NUMBER: _ClassVar[int]
    INCLUDE_HISTORY_FIELD_NUMBER: _ClassVar[int]
    task_run_id: str
    include_history: bool
    def __init__(self, task_run_id: _Optional[str] = ..., include_history: bool = ...) -> None: ...

class SubscribeEventsRequest(_message.Message):
    __slots__ = ("queue_name",)
    QUEUE_NAME_FIELD_NUMBER: _ClassVar[int]
    queue_name: str
    def __init__(self, queue_name: _Optional[str] = ...) -> None: ...
