"""Valka gRPC worker with bidirectional streaming."""

from __future__ import annotations

import asyncio
import json
import logging
import signal
import time
import uuid
from typing import Any, Awaitable, Callable

import grpc
import grpc.aio

from valka.context import TaskContext
from valka.errors import ConnectionError, HandlerError
from valka.retry import RetryPolicy

logger = logging.getLogger("valka.worker")

TaskHandler = Callable[[TaskContext], Awaitable[Any]]

RESULT_RETRY_DELAY = 1.0
SHUTDOWN_ACK_WAIT = 5.0


class ValkaWorker:
    """gRPC bidirectional streaming worker for processing tasks.

    Use the builder pattern::

        worker = (
            ValkaWorker.builder()
            .name("my-worker")
            .server_addr("localhost:50051")
            .queues(["emails", "notifications"])
            .concurrency(4)
            .handler(my_handler)
            .build()
        )
        await worker.run()
    """

    def __init__(
        self,
        *,
        worker_id: str,
        name: str,
        server_addr: str,
        queues: list[str],
        concurrency: int,
        metadata: dict[str, Any] | None,
        handler: TaskHandler,
    ) -> None:
        self._worker_id = worker_id
        self._name = name
        self._server_addr = server_addr
        self._queues = queues
        self._concurrency = concurrency
        self._metadata = metadata
        self._handler = handler

        self._semaphore = asyncio.Semaphore(concurrency)
        self._active_tasks: dict[str, asyncio.Task[None]] = {}
        self._task_contexts: dict[str, TaskContext] = {}
        # task_run_id -> TaskResult, kept until the server answers APPLIED or STALE.
        self._unacked: dict[str, Any] = {}
        self._server_stopping = False
        self._shutting_down = False
        self._shutdown_event = asyncio.Event()
        self._stream: grpc.aio.StreamStreamCall | None = None  # type: ignore[type-arg]

    @staticmethod
    def builder() -> ValkaWorkerBuilder:
        """Create a new worker builder."""
        return ValkaWorkerBuilder()

    @staticmethod
    def create(
        *,
        name: str = "",
        server_addr: str = "localhost:50051",
        queues: list[str],
        concurrency: int = 1,
        metadata: dict[str, Any] | None = None,
        handler: TaskHandler,
    ) -> ValkaWorker:
        """Create a worker directly without builder."""
        worker_id = str(uuid.uuid4())
        return ValkaWorker(
            worker_id=worker_id,
            name=name or f"python-worker-{worker_id[:8]}",
            server_addr=server_addr,
            queues=queues,
            concurrency=concurrency,
            metadata=metadata,
            handler=handler,
        )

    async def run(self) -> None:
        """Start the worker. Blocks until shutdown or unrecoverable error."""
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            loop.add_signal_handler(sig, lambda: asyncio.ensure_future(self.shutdown()))

        retry = RetryPolicy()
        while not self._shutting_down:
            try:
                await self._session(retry)
            except Exception as exc:
                if self._shutting_down:
                    break
                delay = retry.next_delay_seconds()
                logger.warning("Connection lost (%s), reconnecting in %.1fs", exc, delay)
                await asyncio.sleep(delay)

    async def shutdown(self) -> None:
        """Initiate graceful shutdown: stop receiving tasks, let in-flight tasks finish and
        their results be acknowledged, then close the session."""
        if self._shutting_down:
            return
        self._shutting_down = True
        logger.info("Shutting down, draining %d active tasks...", len(self._active_tasks))
        from valka._proto.valka.v1 import worker_pb2

        try:
            await self._send(
                worker_pb2.WorkerRequest(
                    shutdown=worker_pb2.GracefulShutdown(reason="client shutdown")
                )
            )
        except Exception:
            logger.warning("Could not notify the server of the shutdown", exc_info=True)

        if self._active_tasks:
            tasks = list(self._active_tasks.values())
            try:
                await asyncio.wait_for(asyncio.gather(*tasks, return_exceptions=True), timeout=30)
            except asyncio.TimeoutError:
                logger.warning("Drain timeout, cancelling %d tasks", len(self._active_tasks))
                for task in self._active_tasks.values():
                    task.cancel()

        deadline = time.monotonic() + SHUTDOWN_ACK_WAIT
        while self._unacked and time.monotonic() < deadline:
            await asyncio.sleep(0.05)

        if self._stream is not None:
            try:
                await self._stream.done_writing()
            except Exception:
                logger.debug("Session already closed", exc_info=True)
        self._shutdown_event.set()

    async def _session(self, retry: RetryPolicy) -> None:
        from valka._proto.valka.v1 import worker_pb2, worker_pb2_grpc

        channel = grpc.aio.insecure_channel(
            self._server_addr,
            options=[
                ("grpc.keepalive_time_ms", 10_000),
                ("grpc.keepalive_timeout_ms", 5_000),
            ],
        )

        try:
            stub = worker_pb2_grpc.WorkerServiceStub(channel)
            self._stream = stub.Session()

            # Send hello
            meta_str = json.dumps(self._metadata) if self._metadata else ""
            hello = worker_pb2.WorkerRequest(
                hello=worker_pb2.WorkerHello(
                    worker_id=self._worker_id,
                    worker_name=self._name,
                    queues=self._queues,
                    concurrency=self._concurrency,
                    metadata=meta_str,
                )
            )
            await self._stream.write(hello)
            for result in list(self._unacked.values()):
                await self._stream.write(worker_pb2.WorkerRequest(task_result=result))

            retry.reset()
            logger.info(
                "Connected as %s (id=%s, queues=%s, concurrency=%d)",
                self._name,
                self._worker_id,
                self._queues,
                self._concurrency,
            )

            # Start heartbeat task
            heartbeat_task = asyncio.create_task(self._heartbeat_loop())

            self._server_stopping = False
            try:
                async for response in self._stream:
                    await self._handle_response(response, stub)
                    if self._server_stopping:
                        break
            finally:
                heartbeat_task.cancel()
                try:
                    await heartbeat_task
                except asyncio.CancelledError:
                    pass
        finally:
            await channel.close()
            self._stream = None
        if self._server_stopping and not self._shutting_down:
            raise ConnectionError("server shutting down")

    async def _heartbeat_loop(self) -> None:
        from valka._proto.valka.v1 import worker_pb2

        while not self._shutting_down:
            await asyncio.sleep(10)
            if self._stream is None:
                break
            try:
                task_ids = set(self._active_tasks) | {r.task_id for r in self._unacked.values()}
                heartbeat = worker_pb2.WorkerRequest(
                    heartbeat=worker_pb2.Heartbeat(
                        active_task_ids=list(task_ids),
                        timestamp_ms=int(time.time() * 1000),
                    )
                )
                await self._stream.write(heartbeat)
            except Exception:
                break

    async def _handle_response(self, response: Any, stub: Any) -> None:
        kind = response.WhichOneof("response")
        if kind == "task_assignment":
            await self._handle_task_assignment(response.task_assignment, stub)
        elif kind == "task_signal":
            self._handle_task_signal(response.task_signal)
        elif kind == "task_cancellation":
            self._handle_task_cancellation(response.task_cancellation)
        elif kind == "result_ack":
            self._handle_result_ack(response.result_ack)
        elif kind == "server_shutdown":
            logger.info("Server shutdown: %s", response.server_shutdown.reason)
            self._server_stopping = True
        elif kind == "heartbeat_ack":
            pass

    def _handle_result_ack(self, ack: Any) -> None:
        from valka._proto.valka.v1 import worker_pb2

        if ack.status in (worker_pb2.RESULT_STATUS_APPLIED, worker_pb2.RESULT_STATUS_STALE):
            if ack.status == worker_pb2.RESULT_STATUS_STALE:
                logger.warning(
                    "Result for task %s not recorded: the run already ended another way",
                    ack.task_id,
                )
            self._unacked.pop(ack.task_run_id, None)
            return
        run_id = ack.task_run_id
        asyncio.get_running_loop().call_later(
            RESULT_RETRY_DELAY, lambda: asyncio.ensure_future(self._resend(run_id))
        )

    async def _resend(self, run_id: str) -> None:
        from valka._proto.valka.v1 import worker_pb2

        result = self._unacked.get(run_id)
        if result is not None:
            try:
                await self._send(worker_pb2.WorkerRequest(task_result=result))
            except Exception:
                logger.debug("Resend failed; the next session resends", exc_info=True)

    async def _deliver(self, result: Any) -> None:
        """Keep the result until APPLIED or STALE; a later session resends it."""
        from valka._proto.valka.v1 import worker_pb2

        self._unacked[result.task_run_id] = result
        try:
            await self._send(worker_pb2.WorkerRequest(task_result=result))
        except Exception:
            logger.debug("Result send failed; the next session resends", exc_info=True)

    async def _handle_task_assignment(self, assignment: Any, stub: Any) -> None:
        task = asyncio.create_task(self._execute_task(assignment, stub))
        self._active_tasks[assignment.task_id] = task
        task.add_done_callback(lambda _t: self._task_done(assignment.task_id))

    def _handle_task_signal(self, signal: Any) -> None:
        ctx = self._task_contexts.get(signal.task_id)
        if ctx is not None:
            ctx._deliver_signal(signal)

    def _task_done(self, task_id: str) -> None:
        self._active_tasks.pop(task_id, None)
        self._task_contexts.pop(task_id, None)

    async def _execute_task(self, assignment: Any, stub: Any) -> None:
        from valka._proto.valka.v1 import worker_pb2

        async def checkpoint(step: str, output: str) -> None:
            await stub.Checkpoint(
                worker_pb2.CheckpointRequest(
                    task_id=assignment.task_id,
                    task_run_id=assignment.task_run_id,
                    step=step,
                    output=output,
                )
            )

        ctx = TaskContext(
            task_id=assignment.task_id,
            task_run_id=assignment.task_run_id,
            queue_name=assignment.queue_name,
            task_name=assignment.task_name,
            attempt_number=assignment.attempt_number,
            raw_input=assignment.input,
            raw_metadata=assignment.metadata,
            send_fn=self._send,
            checkpoints={c.step: c.output for c in assignment.checkpoints},
            checkpoint_fn=checkpoint,
        )
        self._task_contexts[assignment.task_id] = ctx

        success = False
        retryable = True
        output = ""
        error_message = ""

        # The slot is taken here, not in the receive loop, so acks, signals and
        # cancellations keep flowing while the worker is at capacity.
        async with self._semaphore:
            try:
                result = await self._handler(ctx)
                success = True
                if result is not None:
                    output = json.dumps(result) if not isinstance(result, str) else result
            except HandlerError as exc:
                retryable = exc.retryable
                error_message = str(exc)
                logger.warning("Task %s handler error: %s", assignment.task_id, exc)
            except Exception as exc:
                error_message = str(exc)
                logger.warning("Task %s failed: %s", assignment.task_id, exc)

        await self._deliver(
            worker_pb2.TaskResult(
                task_id=assignment.task_id,
                task_run_id=assignment.task_run_id,
                success=success,
                retryable=retryable,
                output=output,
                error_message=error_message,
            )
        )

    async def _send(self, request: Any) -> None:
        if self._stream is not None:
            await self._stream.write(request)


class ValkaWorkerBuilder:
    """Fluent builder for ValkaWorker."""

    def __init__(self) -> None:
        self._name: str = ""
        self._server_addr: str = "localhost:50051"
        self._queues: list[str] = []
        self._concurrency: int = 1
        self._metadata: dict[str, Any] | None = None
        self._handler: TaskHandler | None = None

    def name(self, name: str) -> ValkaWorkerBuilder:
        """Set the worker display name."""
        self._name = name
        return self

    def server_addr(self, addr: str) -> ValkaWorkerBuilder:
        """Set the gRPC server address (host:port)."""
        self._server_addr = addr
        return self

    def queues(self, queues: list[str]) -> ValkaWorkerBuilder:
        """Set the queues this worker consumes from."""
        self._queues = queues
        return self

    def concurrency(self, n: int) -> ValkaWorkerBuilder:
        """Set max concurrent task handlers."""
        self._concurrency = n
        return self

    def metadata(self, meta: dict[str, Any]) -> ValkaWorkerBuilder:
        """Set optional worker metadata."""
        self._metadata = meta
        return self

    def handler(self, fn: TaskHandler) -> ValkaWorkerBuilder:
        """Set the task handler function."""
        self._handler = fn
        return self

    def build(self) -> ValkaWorker:
        """Build the worker. Raises ValueError if queues or handler missing."""
        if not self._queues:
            raise ValueError("At least one queue is required")
        if self._handler is None:
            raise ValueError("A handler function is required")

        worker_id = str(uuid.uuid4())
        return ValkaWorker(
            worker_id=worker_id,
            name=self._name or f"python-worker-{worker_id[:8]}",
            server_addr=self._server_addr,
            queues=self._queues,
            concurrency=self._concurrency,
            metadata=self._metadata,
            handler=self._handler,
        )
