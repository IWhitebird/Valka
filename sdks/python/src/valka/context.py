"""Task execution context passed to handler functions."""

from __future__ import annotations

import asyncio
import json
import time
from dataclasses import dataclass
from typing import Any, Callable, Awaitable, TYPE_CHECKING

if TYPE_CHECKING:
    from valka._proto.valka.v1 import worker_pb2


@dataclass
class SignalData:
    """Data from a received signal."""

    signal_id: str
    name: str
    payload: str

    def parse_payload(self) -> Any:
        """Parse the signal payload JSON. Returns None if empty."""
        if not self.payload:
            return None
        return json.loads(self.payload)


class TaskContext:
    """Context provided to task handler functions.

    Exposes task metadata, logging, and signal reception methods.
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
        self._signal_queue: asyncio.Queue[Any] = asyncio.Queue()
        self._signal_buffer: list[Any] = []

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

    async def wait_for_signal(self, name: str) -> SignalData:
        """Wait for a signal with the given name. Non-matching signals are buffered."""
        # Check buffer first
        for i, sig in enumerate(self._signal_buffer):
            if sig.signal_name == name:
                self._signal_buffer.pop(i)
                await self._send_signal_ack(sig.signal_id)
                return SignalData(
                    signal_id=sig.signal_id,
                    name=sig.signal_name,
                    payload=sig.payload,
                )

        # Wait for matching signal
        while True:
            sig = await self._signal_queue.get()
            if sig.signal_name == name:
                await self._send_signal_ack(sig.signal_id)
                return SignalData(
                    signal_id=sig.signal_id,
                    name=sig.signal_name,
                    payload=sig.payload,
                )
            self._signal_buffer.append(sig)

    async def receive_signal(self) -> SignalData:
        """Wait for the next signal (any name). Checks buffer first."""
        if self._signal_buffer:
            sig = self._signal_buffer.pop(0)
            await self._send_signal_ack(sig.signal_id)
            return SignalData(
                signal_id=sig.signal_id,
                name=sig.signal_name,
                payload=sig.payload,
            )

        sig = await self._signal_queue.get()
        await self._send_signal_ack(sig.signal_id)
        return SignalData(
            signal_id=sig.signal_id,
            name=sig.signal_name,
            payload=sig.payload,
        )

    def _deliver_signal(self, signal: Any) -> None:
        """Internal: called by the worker to deliver a signal to this context."""
        self._signal_queue.put_nowait(signal)

    async def _send_signal_ack(self, signal_id: str) -> None:
        from valka._proto.valka.v1 import worker_pb2

        request = worker_pb2.WorkerRequest(
            signal_ack=worker_pb2.SignalAck(signal_id=signal_id)
        )
        await self._send_fn(request)

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
