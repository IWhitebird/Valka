"""Step checkpoints: a three-step pipeline whose ``transform`` step fails on the first attempt.

The retry skips the checkpointed ``fetch`` step and resumes at ``transform``.

Usage: python steps.py (requires a Valka server at localhost:50051). Enqueue work with:
    curl -X POST localhost:8989/api/v1/tasks -H 'content-type: application/json' \\
      -d '{"queue_name":"pipeline","task_name":"etl"}'
"""

import asyncio
import logging

from valka import TaskContext, ValkaWorker

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(message)s")


async def handle_task(ctx: TaskContext) -> dict:
    attempt = ctx.attempt_number

    def fetch() -> int:
        print(f"fetch runs on attempt {attempt}")
        return 3

    rows = await ctx.step("fetch", fetch)

    def transform() -> int:
        if attempt == 1:
            raise RuntimeError("transform failed; the retry resumes here")
        return rows * 2

    doubled = await ctx.step("transform", transform)
    stored = await ctx.step("store", lambda: f"stored {doubled} rows")
    return {"rows": rows, "doubled": doubled, "stored": stored, "attempt": attempt}


async def main() -> None:
    worker = (
        ValkaWorker.builder()
        .name("steps-worker")
        .server_addr("localhost:50051")
        .queues(["pipeline"])
        .handler(handle_task)
        .build()
    )
    await worker.run()


if __name__ == "__main__":
    asyncio.run(main())
