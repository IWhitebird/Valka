from valka.v1 import common_pb2 as _common_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class TaskEvent(_message.Message):
    __slots__ = ("event_id", "task_id", "queue_name", "previous_status", "new_status", "worker_id", "node_id", "attempt_number", "error_message", "timestamp_ms")
    EVENT_ID_FIELD_NUMBER: _ClassVar[int]
    TASK_ID_FIELD_NUMBER: _ClassVar[int]
    QUEUE_NAME_FIELD_NUMBER: _ClassVar[int]
    PREVIOUS_STATUS_FIELD_NUMBER: _ClassVar[int]
    NEW_STATUS_FIELD_NUMBER: _ClassVar[int]
    WORKER_ID_FIELD_NUMBER: _ClassVar[int]
    NODE_ID_FIELD_NUMBER: _ClassVar[int]
    ATTEMPT_NUMBER_FIELD_NUMBER: _ClassVar[int]
    ERROR_MESSAGE_FIELD_NUMBER: _ClassVar[int]
    TIMESTAMP_MS_FIELD_NUMBER: _ClassVar[int]
    event_id: str
    task_id: str
    queue_name: str
    previous_status: _common_pb2.TaskStatus
    new_status: _common_pb2.TaskStatus
    worker_id: str
    node_id: str
    attempt_number: int
    error_message: str
    timestamp_ms: int
    def __init__(self, event_id: _Optional[str] = ..., task_id: _Optional[str] = ..., queue_name: _Optional[str] = ..., previous_status: _Optional[_Union[_common_pb2.TaskStatus, str]] = ..., new_status: _Optional[_Union[_common_pb2.TaskStatus, str]] = ..., worker_id: _Optional[str] = ..., node_id: _Optional[str] = ..., attempt_number: _Optional[int] = ..., error_message: _Optional[str] = ..., timestamp_ms: _Optional[int] = ...) -> None: ...
