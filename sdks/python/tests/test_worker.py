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

