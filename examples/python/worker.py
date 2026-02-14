"""Example: Process tasks using the Valka Python SDK worker."""

import asyncio
import logging

from valka import ValkaWorker, TaskContext

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(message)s")


async def handle_task(ctx: TaskContext) -> dict:
    """Handle an email task."""
    data = ctx.input()
    to = data.get("to", "unknown") if data else "unknown"

    await ctx.log(f"Processing email task: {ctx.task_name}")
    await ctx.debug(f"Attempt #{ctx.attempt_number}")

    # Simulate work
    await asyncio.sleep(1)

    await ctx.log(f"Email sent to {to}")
    return {"delivered_to": to, "status": "sent"}


async def main() -> None:
    worker = (
        ValkaWorker.builder()
        .name("python-email-worker")
        .server_addr("localhost:50051")
        .queues(["emails", "notifications"])
        .concurrency(4)
        .handler(handle_task)
        .build()
    )
    await worker.run()


if __name__ == "__main__":
    asyncio.run(main())
