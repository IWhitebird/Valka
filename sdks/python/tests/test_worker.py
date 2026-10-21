import asyncio

from valka import worker as worker_mod
from valka._proto.valka.v1 import worker_pb2
from valka.worker import ValkaWorker


class FakeStream:
    def __init__(self):
        self.sent = []
        self.closed = False

    async def write(self, request):
        self.sent.append(request)

    async def done_writing(self):
        self.closed = True


def make_worker(handler=None, concurrency=1):
    async def default(ctx):
        return {"ok": True}

    w = ValkaWorker.create(queues=["q"], concurrency=concurrency, handler=handler or default)
    w._stream = FakeStream()
    return w


def sent(w, kind):
    return [r for r in w._stream.sent if r.WhichOneof("request") == kind]


def result(run):
    return worker_pb2.TaskResult(task_id="task-" + run, task_run_id=run, success=True)


def ack(run, status):
    return worker_pb2.ResultAck(task_id="task-" + run, task_run_id=run, status=status)


def test_result_is_kept_until_applied():
    async def run():
        w = make_worker()
        await w._deliver(result("r1"))
        assert len(sent(w, "task_result")) == 1
        assert "r1" in w._unacked
        w._handle_result_ack(ack("r1", worker_pb2.RESULT_STATUS_APPLIED))
        assert not w._unacked

    asyncio.run(run())


def test_stale_ack_clears_the_result():
    async def run():
        w = make_worker()
        await w._deliver(result("r1"))
        w._handle_result_ack(ack("r1", worker_pb2.RESULT_STATUS_STALE))
        assert not w._unacked

    asyncio.run(run())


def test_retry_ack_resends_after_a_delay(monkeypatch):
    monkeypatch.setattr(worker_mod, "RESULT_RETRY_DELAY", 0.01)

    async def run():
        w = make_worker()
        await w._deliver(result("r1"))
        w._handle_result_ack(ack("r1", worker_pb2.RESULT_STATUS_RETRY))
        await asyncio.sleep(0.05)
        assert "r1" in w._unacked
        assert len(sent(w, "task_result")) == 2

    asyncio.run(run())


def test_server_shutdown_means_reconnect_not_exit():
    async def run():
        w = make_worker()
        response = worker_pb2.WorkerResponse(
            server_shutdown=worker_pb2.ServerShutdown(reason="restart")
        )
        await w._handle_response(response)
        assert w._server_stopping
        assert not w._shutting_down

    asyncio.run(run())


def test_assignment_never_blocks_the_receive_loop_at_capacity():
    gate = asyncio.Event()

    async def handler(ctx):
        await gate.wait()
        return {"task": ctx.task_id}

    async def run():
        w = make_worker(handler, concurrency=1)
        for task_id in ("a", "b"):
            assignment = worker_pb2.TaskAssignment(task_id=task_id, task_run_id=task_id + "-run")
            await asyncio.wait_for(w._handle_task_assignment(assignment), 0.1)
        assert set(w._active_tasks) == {"a", "b"}
        gate.set()
        await asyncio.gather(*w._active_tasks.values())
        assert {r.task_result.task_run_id for r in sent(w, "task_result")} == {"a-run", "b-run"}

    asyncio.run(run())


def test_graceful_shutdown_tells_the_server_and_closes_the_session():
    async def run():
        w = make_worker()
        await w.shutdown()
        assert len(sent(w, "shutdown")) == 1
        assert w._stream.closed

    asyncio.run(run())


def test_checkpoint_without_a_session_fails_fast():
    async def handler(ctx):
        await ctx.checkpoint("step", 1)

    async def run():
        w = make_worker(handler)
        assignment = worker_pb2.TaskAssignment(task_id="t", task_run_id="t-run")
        await w._handle_task_assignment(assignment)
        await asyncio.gather(*w._active_tasks.values())
        (res,) = [r.task_result for r in sent(w, "task_result")]
        assert not res.success
        assert "not connected" in res.error_message

    asyncio.run(run())


def test_hello_lists_runs_still_executing():
    gate = asyncio.Event()

    async def handler(ctx):
        await gate.wait()
        return {}

    async def run():
        w = make_worker(handler)
        assert list(w._hello().hello.running) == []
        assignment = worker_pb2.TaskAssignment(task_id="a", task_run_id="a-run", attempt_number=3)
        await w._handle_task_assignment(assignment)
        running = w._hello().hello.running
        assert [(r.task_id, r.task_run_id, r.attempt_number) for r in running] == [
            ("a", "a-run", 3)
        ]
        gate.set()
        await asyncio.gather(*w._active_tasks.values())
        await asyncio.sleep(0)
        assert list(w._hello().hello.running) == []

    asyncio.run(run())


def test_cancellation_stops_the_handler_and_reports_it():
    cancelled = []

    async def handler(ctx):
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            cancelled.append(ctx.task_run_id)
            raise

    async def run():
        w = make_worker(handler)
        assignment = worker_pb2.TaskAssignment(task_id="t", task_run_id="t-run")
        await w._handle_task_assignment(assignment)
        await asyncio.sleep(0)
        cancel = worker_pb2.WorkerResponse(
            task_cancellation=worker_pb2.TaskCancellation(task_id="t", reason="Cancelled by user")
        )
        await w._handle_response(cancel)
        # Not listed on reconnect any more: a stale run would be cancelled again and again.
        assert list(w._hello().hello.running) == []
        await asyncio.wait_for(asyncio.gather(*w._active_tasks.values()), 1)
        assert cancelled == ["t-run"]
        (res,) = [r.task_result for r in sent(w, "task_result")]
        assert res.task_run_id == "t-run" and not res.success and not res.retryable
        assert "cancelled" in res.error_message
        assert w._executing == 0 and not w._active_tasks

    asyncio.run(run())


def test_a_newer_run_replaces_an_older_one_of_the_same_task():
    release = asyncio.Event()

    async def handler(ctx):
        if ctx.task_run_id == "r1":
            await asyncio.Event().wait()
        await release.wait()
        return {}

    async def run():
        w = make_worker(handler, concurrency=2)
        await w._handle_task_assignment(worker_pb2.TaskAssignment(task_id="t", task_run_id="r1"))
        await asyncio.sleep(0)
        await w._handle_task_assignment(worker_pb2.TaskAssignment(task_id="t", task_run_id="r2"))
        for _ in range(5):
            await asyncio.sleep(0)
        # The stale run is stopped without a result, and leaves the newer run's state alone.
        assert w._executing == 1
        assert [r.task_run_id for r in w._hello().hello.running] == ["r2"]
        assert w._task_contexts["t"].task_run_id == "r2"
        assert not sent(w, "task_result")
        release.set()
        await asyncio.wait_for(asyncio.gather(*w._active_tasks.values()), 1)
        assert [r.task_result.task_run_id for r in sent(w, "task_result")] == ["r2"]

    asyncio.run(run())


class BrokenStream(FakeStream):
    async def write(self, request):
        raise RuntimeError("RPC already finished")


def test_logs_never_fail_the_handler():
    async def handler(ctx):
        await ctx.log("working")
        return {"ok": True}

    async def run():
        w = make_worker(handler)
        w._stream = BrokenStream()
        await w._handle_task_assignment(worker_pb2.TaskAssignment(task_id="t", task_run_id="r"))
        await asyncio.gather(*w._active_tasks.values())
        assert w._unacked["r"].success

    asyncio.run(run())


# --- Sessions against scripted fake nodes ---


class FakeCall:
    """A Session call: records writes; answers come from the node's script."""

    def __init__(self, node):
        self.node = node
        self.sent = []
        self.answers = asyncio.Queue()

    async def write(self, request):
        self.sent.append(request)
        for answer in self.node.answer(request):
            self.answers.put_nowait(answer)

    async def done_writing(self):
        self.answers.put_nowait(None)

    def __aiter__(self):
        return self

    async def __anext__(self):
        answer = await self.answers.get()
        if answer is None:
            raise StopAsyncIteration
        return answer


class FakeNode:
    def __init__(self, on_hello=(), leave_on_drain=False, ack_results=True):
        self.on_hello = list(on_hello)
        self.leave_on_drain = leave_on_drain
        self.ack_results = ack_results
        self.calls = []

    def answer(self, request):
        kind = request.WhichOneof("request")
        if kind == "hello":
            return self.on_hello
        if kind == "shutdown" and self.leave_on_drain:
            bye = worker_pb2.ServerShutdown(reason="restart")
            return [worker_pb2.WorkerResponse(server_shutdown=bye), None]
        if kind == "task_result" and self.ack_results:
            r = request.task_result
            ack = worker_pb2.ResultAck(
                task_id=r.task_id,
                task_run_id=r.task_run_id,
                status=worker_pb2.RESULT_STATUS_APPLIED,
            )
            return [worker_pb2.WorkerResponse(result_ack=ack)]
        return []

    def requests(self):
        return [r for call in self.calls for r in call.sent]


def install_nodes(monkeypatch, nodes):
    from valka._proto.valka.v1 import worker_pb2_grpc

    class FakeChannel:
        def __init__(self, addr):
            self.addr = addr

        async def close(self):
            pass

    class FakeStub:
        def __init__(self, channel):
            self.node = nodes[channel.addr]

        def Session(self, metadata=None):  # noqa: N802 - gRPC method name
            call = FakeCall(self.node)
            self.node.calls.append(call)
            return call

    monkeypatch.setattr(
        worker_mod.grpc.aio, "insecure_channel", lambda addr, options: FakeChannel(addr)
    )
    monkeypatch.setattr(worker_pb2_grpc, "WorkerServiceStub", FakeStub)


def test_backoff_resets_only_once_the_server_answers(monkeypatch):
    silent = FakeNode()
    answering = FakeNode(
        on_hello=[worker_pb2.WorkerResponse(heartbeat_ack=worker_pb2.HeartbeatAck())]
    )
    install_nodes(monkeypatch, {"silent": silent, "answering": answering})

    async def run():
        retry = worker_mod.RetryPolicy()
        retry._attempt = 5
        w = ValkaWorker.create(queues=["q"], server_addrs=["silent"], handler=lambda ctx: None)
        task = asyncio.create_task(w._session(retry))
        await asyncio.sleep(0.01)
        silent.calls[0].answers.put_nowait(None)  # the stream ends without an answer
        try:
            await task
        except Exception:
            pass
        assert retry._attempt == 5

        w = ValkaWorker.create(queues=["q"], server_addrs=["answering"], handler=lambda ctx: None)
        task = asyncio.create_task(w._session(retry))
        await asyncio.sleep(0.01)
        answering.calls[0].answers.put_nowait(None)
        try:
            await task
        except Exception:
            pass
        assert retry._attempt == 0

    asyncio.run(run())


def test_a_bad_message_does_not_end_the_session(monkeypatch):
    node = FakeNode()
    install_nodes(monkeypatch, {"n": node})

    async def run():
        w = ValkaWorker.create(queues=["q"], server_addrs=["n"], handler=lambda ctx: None)
        handled = []

        async def flaky(response):
            handled.append(response.WhichOneof("response"))
            if len(handled) == 1:
                raise ValueError("boom")

        w._handle_response = flaky
        task = asyncio.create_task(w._session(worker_mod.RetryPolicy()))
        await asyncio.sleep(0.01)
        ack = worker_pb2.WorkerResponse(heartbeat_ack=worker_pb2.HeartbeatAck())
        node.calls[0].answers.put_nowait(ack)
        node.calls[0].answers.put_nowait(ack)
        node.calls[0].answers.put_nowait(None)
        try:
            await task
        except Exception:
            pass
        assert handled == ["heartbeat_ack", "heartbeat_ack"]

    asyncio.run(run())


def test_a_drain_survives_a_server_shutdown(monkeypatch):
    assignment = worker_pb2.WorkerResponse(
        task_assignment=worker_pb2.TaskAssignment(task_id="t1", task_run_id="r1", attempt_number=1)
    )
    a = FakeNode(on_hello=[assignment], leave_on_drain=True)
    b = FakeNode()
    install_nodes(monkeypatch, {"a": a, "b": b})

    async def run():
        started = asyncio.Event()
        release = asyncio.Event()

        async def handler(ctx):
            started.set()
            await release.wait()
            return {"ok": True}

        w = ValkaWorker.create(queues=["q"], server_addrs=["a", "b"], handler=handler)
        runner = asyncio.create_task(w.run())
        await asyncio.wait_for(started.wait(), 2)
        stopping = asyncio.create_task(w.shutdown())

        async def b_heard_the_drain():
            while [r.WhichOneof("request") for r in b.requests()][:2] != ["hello", "shutdown"]:
                await asyncio.sleep(0.01)

        await asyncio.wait_for(b_heard_the_drain(), 5)
        (hello, _) = b.requests()[:2]
        assert [r.task_run_id for r in hello.hello.running] == ["r1"]
        assert not runner.done(), "the drain waits for the running handler"

        release.set()
        await asyncio.wait_for(runner, 5)
        await stopping
        results = {
            name: [r.task_result.task_run_id for r in n.requests() if r.HasField("task_result")]
            for name, n in (("a", a), ("b", b))
        }
        assert results == {"a": [], "b": ["r1"]}

    asyncio.run(run())
