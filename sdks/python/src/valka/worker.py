"""Valka gRPC worker with bidirectional streaming."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import signal
import time
import uuid
from collections.abc import Awaitable, Callable
from typing import Any

import grpc
import grpc.aio

from valka.context import TaskContext
from valka.errors import ConnectionError, HandlerError
from valka.retry import RetryPolicy

logger = logging.getLogger("valka.worker")

TaskHandler = Callable[[TaskContext], Awaitable[Any]]

RESULT_RETRY_DELAY = 1.0
# A drain gives running handlers DRAIN_TIMEOUT to finish, then their results
# SHUTDOWN_ACK_WAIT to be acknowledged.
DRAIN_TIMEOUT = 30.0
SHUTDOWN_ACK_WAIT = 10.0
DRAIN_POLL = 0.05
# Reconnect delay cap while draining: results are waiting for a connection.
DRAIN_RECONNECT_DELAY = 1.0


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
        server_addrs: list[str] | None = None,
        api_key: str | None = None,
    ) -> None:
        self._worker_id = worker_id
        self._name = name
        # Several nodes of one cluster: connect to one, move to the next when it drops.
        self._server_addrs = server_addrs or [server_addr]
        self._next_addr = 0
        key = api_key if api_key is not None else os.environ.get("VALKA_API_KEY", "")
        self._call_metadata = (("authorization", f"Bearer {key}"),) if key else ()
        self._queues = queues
        self._concurrency = concurrency
        self._metadata = metadata
        self._handler = handler

        self._semaphore = asyncio.Semaphore(concurrency)
        # Per-task state holds the latest run of each task. An older run of the same task
        # still finishing here is stale (the server starts a new run only after the old one
        # ended): it is stopped, and its cleanup never removes the newer run's state.
        self._active_tasks: dict[str, asyncio.Task[None]] = {}
        # Runs still executing, by task id: listed in every WorkerHello so the server can
        # reattach them to the new connection (or adopt or cancel them).
        self._runs: dict[str, tuple[str, int]] = {}
        self._task_contexts: dict[str, TaskContext] = {}
        # Handlers running or waiting for a slot, until their result is in _unacked.
        self._executing = 0
        # Runs the server cancelled (their handler is being stopped) and runs whose handler
        # is over and whose result is being delivered (no longer cancellable).
        self._cancelled_runs: set[str] = set()
        self._delivering: set[str] = set()
        # task_run_id -> TaskResult, kept until the server answers APPLIED or STALE.
        self._unacked: dict[str, Any] = {}
        self._server_stopping = False
        self._stub: Any = None
        self._shutting_down = False
        # Set once a graceful shutdown is over: run() then returns.
        self._drained = asyncio.Event()
        # The current session's stream, set only once its WorkerHello is written.
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
        server_addrs: list[str] | None = None,
        api_key: str | None = None,
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
            server_addrs=server_addrs,
            api_key=api_key,
            queues=queues,
            concurrency=concurrency,
            metadata=metadata,
            handler=handler,
        )

    async def run(self) -> None:
        """Start the worker. Blocks until a graceful shutdown is over.

        A shutdown (SIGINT, SIGTERM or :meth:`shutdown`) drains: the server stops sending
        tasks, running handlers finish and their results are acknowledged. A connection
        lost meanwhile is re-established (on the next node) and the drain goes on there.
        """
        loop = asyncio.get_running_loop()
        signals = (signal.SIGINT, signal.SIGTERM)
        for sig in signals:
            loop.add_signal_handler(sig, lambda: asyncio.ensure_future(self.shutdown()))

        retry = RetryPolicy()
        try:
            while not self._drained.is_set():
                try:
                    await self._session(retry)
                except Exception as exc:
                    if self._drained.is_set():
                        break
                    # Next time, try the next node: any node takes any worker.
                    self._next_addr += 1
                    delay = retry.next_delay_seconds()
                    if self._shutting_down:
                        delay = min(delay, DRAIN_RECONNECT_DELAY)
                    logger.warning("Connection lost (%s), reconnecting in %.1fs", exc, delay)
                    try:
                        await asyncio.wait_for(self._drained.wait(), timeout=delay)
                    except asyncio.TimeoutError:
                        pass
        finally:
            for sig in signals:
                loop.remove_signal_handler(sig)

    async def shutdown(self) -> None:
        """Initiate graceful shutdown: stop receiving tasks, let in-flight tasks finish and
        their results be acknowledged, then close the session.

        The drain outlives connections: a session that ends meanwhile is re-established and
        told about the drain again."""
        if self._shutting_down:
            return
        self._shutting_down = True
        logger.info("Shutting down, draining %d active tasks...", self._executing)
        from valka._proto.valka.v1 import worker_pb2

        try:
            # The current session, if any; a later one sends its own after its hello.
            await self._send(
                worker_pb2.WorkerRequest(
                    shutdown=worker_pb2.GracefulShutdown(reason="client shutdown")
                )
            )
        except Exception:
            logger.warning("Could not notify the server of the shutdown", exc_info=True)

        # Also waits for tasks the server sent before it saw the shutdown.
        deadline = time.monotonic() + DRAIN_TIMEOUT
        while self._executing and time.monotonic() < deadline:
            await asyncio.sleep(DRAIN_POLL)
        if self._executing:
            logger.warning("Drain timeout, cancelling %d tasks", self._executing)
            for task in list(self._active_tasks.values()):
                task.cancel()

        deadline = time.monotonic() + SHUTDOWN_ACK_WAIT
        while self._unacked and time.monotonic() < deadline:
            await asyncio.sleep(DRAIN_POLL)

        self._drained.set()
        if self._stream is not None:
            try:
                await self._stream.done_writing()
            except Exception:
                logger.debug("Session already closed", exc_info=True)

    async def _session(self, retry: RetryPolicy) -> None:
        """One connection. Returns once a drain is over; raises when the session ends
        otherwise (the caller reconnects)."""
        from valka._proto.valka.v1 import worker_pb2, worker_pb2_grpc

        channel = grpc.aio.insecure_channel(
            self._server_addrs[self._next_addr % len(self._server_addrs)],
            options=[
                ("grpc.keepalive_time_ms", 10_000),
                ("grpc.keepalive_timeout_ms", 5_000),
                # The server bounds what a task carries (its payload limit); gRPC's 4 MiB
                # default would break the stream on a large assignment.
                ("grpc.max_receive_message_length", -1),
            ],
        )

        try:
            stub = worker_pb2_grpc.WorkerServiceStub(channel)
            stream = stub.Session(metadata=self._call_metadata)

            await stream.write(self._hello())
            if self._shutting_down:
                # A drain carried over from an earlier session: no new tasks here either.
                await stream.write(
                    worker_pb2.WorkerRequest(
                        shutdown=worker_pb2.GracefulShutdown(reason="client shutdown")
                    )
                )
            # Register this session before resending unacked results: a result delivered
            # concurrently is then either in the snapshot below or sent on this session.
            self._stub = stub
            self._stream = stream
            for result in list(self._unacked.values()):
                await stream.write(worker_pb2.WorkerRequest(task_result=result))

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
            answered = False
            try:
                async for response in stream:
                    if not answered:
                        # The server took the session: back off from scratch after it ends.
                        answered = True
                        retry.reset()
                    try:
                        await self._handle_response(response)
                    except Exception:
                        logger.exception("Could not handle a message from the server")
                    if self._server_stopping:
                        break
            finally:
                heartbeat_task.cancel()
                try:
                    await heartbeat_task
                except asyncio.CancelledError:
                    pass
        finally:
            self._stream = None
            self._stub = None
            await channel.close()
        if self._drained.is_set():
            return
        if self._server_stopping:
            # Reconnect to another node; a drain in progress goes on there.
            raise ConnectionError("server shutting down")
        raise ConnectionError("the server closed the session")

    async def _heartbeat_loop(self) -> None:
        from valka._proto.valka.v1 import worker_pb2

        while not self._drained.is_set():
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

    async def _handle_response(self, response: Any) -> None:
        kind = response.WhichOneof("response")
        if kind == "task_assignment":
            await self._handle_task_assignment(response.task_assignment)
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

    def _hello(self) -> Any:
        """Open a session, listing the runs still executing from before this connection."""
        from valka._proto.valka.v1 import worker_pb2

        meta_str = json.dumps(self._metadata) if self._metadata else ""
        return worker_pb2.WorkerRequest(
            hello=worker_pb2.WorkerHello(
                worker_id=self._worker_id,
                worker_name=self._name,
                queues=self._queues,
                concurrency=self._concurrency,
                metadata=meta_str,
                running=[
                    worker_pb2.RunningTask(
                        task_id=task_id, task_run_id=run_id, attempt_number=attempt
                    )
                    for task_id, (run_id, attempt) in self._runs.items()
                ],
            )
        )

    async def _handle_task_assignment(self, assignment: Any) -> None:
        task_id, run_id = assignment.task_id, assignment.task_run_id
        previous = self._active_tasks.get(task_id)
        if previous is not None and not previous.done():
            # An older run of this task is still here: it is stale, stop it.
            previous.cancel()
        self._runs[task_id] = (run_id, assignment.attempt_number)
        self._executing += 1
        task = asyncio.create_task(self._execute_task(assignment))
        self._active_tasks[task_id] = task
        task.add_done_callback(lambda t: self._task_done(task_id, run_id, t))

    def _handle_task_signal(self, signal: Any) -> None:
        ctx = self._task_contexts.get(signal.task_id)
        if ctx is not None:
            ctx._deliver_signal(signal)

    def _handle_task_cancellation(self, cancellation: Any) -> None:
        """The server cancelled the task (or found its run stale): stop the handler and
        report it, so the server frees the slot. The run is no longer listed on reconnect."""
        task_id = cancellation.task_id
        logger.info("Task %s cancelled by the server: %s", task_id, cancellation.reason)
        run = self._runs.pop(task_id, None)
        task = self._active_tasks.get(task_id)
        if run is None or task is None or task.done() or run[0] in self._delivering:
            return
        self._cancelled_runs.add(run[0])
        task.cancel()

    def _task_done(self, task_id: str, run_id: str, task: asyncio.Task[None]) -> None:
        self._executing -= 1
        self._cancelled_runs.discard(run_id)
        # Only this run's state: a newer run of the same task may have replaced it.
        if self._active_tasks.get(task_id) is task:
            del self._active_tasks[task_id]
        if self._runs.get(task_id, ("",))[0] == run_id:
            del self._runs[task_id]
        ctx = self._task_contexts.get(task_id)
        if ctx is not None and ctx.task_run_id == run_id:
            del self._task_contexts[task_id]
        if not task.cancelled() and task.exception() is not None:
            logger.error("Task %s run %s crashed", task_id, run_id, exc_info=task.exception())

    async def _execute_task(self, assignment: Any) -> None:
        from valka._proto.valka.v1 import worker_pb2

        async def checkpoint(step: str, output: str) -> None:
            # Through the current session, so a task that outlives a reconnect can still
            # checkpoint.
            stub = self._stub
            if stub is None:
                raise ConnectionError("not connected to the server")
            await stub.Checkpoint(
                worker_pb2.CheckpointRequest(
                    task_id=assignment.task_id,
                    task_run_id=assignment.task_run_id,
                    step=step,
                    output=output,
                ),
                metadata=self._call_metadata,
            )

        ctx = TaskContext(
            task_id=assignment.task_id,
            task_run_id=assignment.task_run_id,
            queue_name=assignment.queue_name,
            task_name=assignment.task_name,
            attempt_number=assignment.attempt_number,
            raw_input=assignment.input,
            raw_metadata=assignment.metadata,
            send_fn=self._send_best_effort,
            checkpoints={c.step: c.output for c in assignment.checkpoints},
            checkpoint_fn=checkpoint,
        )
        self._task_contexts[assignment.task_id] = ctx

        success = False
        retryable = True
        output = ""
        error_message = ""
        run_id = assignment.task_run_id

        try:
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
        except asyncio.CancelledError:
            if run_id not in self._cancelled_runs:
                raise  # superseded by a newer run, or the drain timed out: no result
            # Cancelled by the server: say so, which frees the run's slot there.
            current = asyncio.current_task()
            if current is not None and hasattr(current, "uncancel"):
                current.uncancel()
            success, retryable, output = False, False, ""
            error_message = "cancelled by the server"

        # The handler is over: a cancellation can no longer interrupt the delivery.
        self._delivering.add(run_id)
        try:
            await self._deliver(
                worker_pb2.TaskResult(
                    task_id=assignment.task_id,
                    task_run_id=run_id,
                    success=success,
                    retryable=retryable,
                    output=output,
                    error_message=error_message,
                )
            )
        finally:
            self._delivering.discard(run_id)

    async def _send(self, request: Any) -> None:
        if self._stream is not None:
            await self._stream.write(request)

    async def _send_best_effort(self, request: Any) -> None:
        """Logs and signal acks: dropped while the session is down, never an error for the
        handler (a dropped signal ack is redelivered by the server)."""
        try:
            await self._send(request)
        except Exception:
            logger.debug("Dropped a message: the session is down", exc_info=True)


class ValkaWorkerBuilder:
    """Fluent builder for ValkaWorker."""

    def __init__(self) -> None:
        self._name: str = ""
        self._server_addrs: list[str] = ["localhost:50051"]
        self._api_key: str | None = None
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
        self._server_addrs = [addr]
        return self

    def server_addrs(self, addrs: list[str]) -> ValkaWorkerBuilder:
        """Set several nodes of one cluster (host:port each). The worker connects to one and
        moves to the next when its connection drops; running tasks reattach wherever it
        lands."""
        if not addrs:
            raise ValueError("At least one server address is required")
        self._server_addrs = list(addrs)
        return self

    def api_key(self, key: str) -> ValkaWorkerBuilder:
        """Set the API key, sent as ``authorization: Bearer <key>`` (default: the
        ``VALKA_API_KEY`` environment variable)."""
        self._api_key = key
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
            server_addr=self._server_addrs[0],
            server_addrs=self._server_addrs,
            api_key=self._api_key,
            queues=self._queues,
            concurrency=self._concurrency,
            metadata=self._metadata,
            handler=self._handler,
        )
