"""Task execution context passed to handler functions."""

from __future__ import annotations

import json
import time
from typing import Any, Callable, Awaitable, TYPE_CHECKING

if TYPE_CHECKING:
    from valka._proto.valka.v1 import worker_pb2


class TaskContext:
    """Context provided to task handler functions.

    Exposes task metadata and logging methods. Logging is fire-and-forget
    via the provided send callback.
    """

    def __init__(
        self,
        *,
        task_id: str,
        task_run_id: str,
        queue_name: str,
        task_name: str,
        attempt_number: int,
        raw_input: str,
        raw_metadata: str,
        send_fn: Callable[[worker_pb2.WorkerRequest], Awaitable[None]],
    ) -> None:
        self.task_id = task_id
        self.task_run_id = task_run_id
        self.queue_name = queue_name
        self.task_name = task_name
        self.attempt_number = attempt_number
        self._raw_input = raw_input
        self._raw_metadata = raw_metadata
        self._send_fn = send_fn

    def input(self) -> Any:
        """Parse and return the task input JSON. Returns None if empty."""
        if not self._raw_input:
            return None
        return json.loads(self._raw_input)

    def metadata(self) -> dict[str, Any]:
        """Parse and return the task metadata JSON. Returns {} if empty."""
        if not self._raw_metadata:
            return {}
        return json.loads(self._raw_metadata)

    async def log(self, message: str) -> None:
        """Send an INFO log entry."""
        await self._log_at_level(2, message)

    async def debug(self, message: str) -> None:
        """Send a DEBUG log entry."""
        await self._log_at_level(1, message)

    async def warn(self, message: str) -> None:
        """Send a WARN log entry."""
        await self._log_at_level(3, message)

    async def error(self, message: str) -> None:
        """Send an ERROR log entry."""
        await self._log_at_level(4, message)

    async def _log_at_level(self, level: int, message: str) -> None:
        from valka._proto.valka.v1 import worker_pb2

        entry = worker_pb2.LogEntry(
            task_run_id=self.task_run_id,
            timestamp_ms=int(time.time() * 1000),
            level=level,
            message=message,
        )
        request = worker_pb2.WorkerRequest(
            log_batch=worker_pb2.LogBatch(entries=[entry])
        )
        await self._send_fn(request)
