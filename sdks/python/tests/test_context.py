import asyncio

import pytest

from valka.context import TaskContext


def make_context(checkpoints=None, fail=None):
    saved = []

    async def send(_request):
        pass

    async def checkpoint(step, output):
        if fail is not None:
            raise fail
        saved.append((step, output))

    ctx = TaskContext(
        task_id="task-1",
        task_run_id="run-1",
        queue_name="q",
        task_name="t",
        attempt_number=2,
        raw_input="{}",
        raw_metadata="{}",
        send_fn=send,
        checkpoints=checkpoints or {},
        checkpoint_fn=checkpoint,
    )
    return ctx, saved


def test_step_skips_checkpointed_step():
    ctx, saved = make_context({"fetch": '{"rows": 3}'})
    calls = []

    async def fetch():
        calls.append(1)
        return {"rows": 99}

    assert asyncio.run(ctx.step("fetch", fetch)) == {"rows": 3}
    assert calls == [] and saved == []


def test_step_runs_sync_and_async_functions_and_checkpoints_them():
    ctx, saved = make_context()

    async def run():
        a = await ctx.step("sync", lambda: [1, 2])
        b = await ctx.step("async", lambda: asyncio.sleep(0, result="done"))
        again = await ctx.step("sync", lambda: pytest.fail("must not rerun"))
        return a, b, again

    assert asyncio.run(run()) == ([1, 2], "done", [1, 2])
    assert saved == [("sync", "[1, 2]"), ("async", '"done"')]


def test_step_error_is_not_checkpointed():
    ctx, saved = make_context()

    def boom():
        raise ValueError("bad input")

    with pytest.raises(ValueError):
        asyncio.run(ctx.step("parse", boom))
    assert saved == [] and ctx.checkpoint_value("parse") is None


def test_step_fails_when_checkpoint_is_not_persisted():
    ctx, _ = make_context(fail=ConnectionError("unavailable"))

    with pytest.raises(ConnectionError):
        asyncio.run(ctx.step("upload", lambda: "ok"))
    assert ctx.checkpoint_value("upload", "missing") == "missing"


def test_checkpoint_value_distinguishes_null_from_missing():
    ctx, _ = make_context({"done": "null"})
    sentinel = object()
    assert ctx.checkpoint_value("done", sentinel) is None
    assert ctx.checkpoint_value("absent", sentinel) is sentinel
    assert asyncio.run(ctx.step("done", lambda: pytest.fail("must not rerun"))) is None
