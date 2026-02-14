# Valka Python SDK

Python SDK for the [Valka](https://github.com/your-org/valka) distributed task queue.

## Installation

```bash
pip install valka
```

## Quick Start

### Client — Create and manage tasks (REST)

```python
import asyncio
from valka import ValkaClient

async def main():
    async with ValkaClient("http://localhost:8080") as client:
        # Create a task
        task = await client.create_task(
            queue_name="emails",
            task_name="send-welcome",
            input={"to": "user@example.com", "subject": "Welcome!"},
        )
        print(f"Created task {task['id']}")

        # Get task status
        task = await client.get_task(task["id"])
        print(f"Status: {task['status']}")

        # List tasks
        tasks = await client.list_tasks(queue_name="emails", limit=10)
        print(f"Found {len(tasks)} tasks")

asyncio.run(main())
```

### Worker — Process tasks (gRPC)

```python
import asyncio
from valka import ValkaWorker, TaskContext

async def handle_task(ctx: TaskContext) -> dict:
    data = ctx.input()
    await ctx.log(f"Sending email to {data['to']}")
    # ... do work ...
    return {"delivered": True}

async def main():
    worker = (
        ValkaWorker.builder()
        .name("email-worker")
        .server_addr("localhost:50051")
        .queues(["emails"])
        .concurrency(4)
        .handler(handle_task)
        .build()
    )
    await worker.run()

asyncio.run(main())
```

## Requirements

- Python 3.10+
- A running Valka server

## Proto Generation

Proto stubs are pre-generated. To regenerate:

```bash
pip install grpcio-tools
bash generate_proto.sh
```
