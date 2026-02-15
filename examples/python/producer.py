"""Example: Create tasks using the Valka Python SDK."""

import asyncio

from valka import ValkaClient


async def main() -> None:
    async with ValkaClient("http://localhost:8989") as client:
        # Health check
        health = await client.health_check()
        print(f"Server health: {health}")

        # Create several tasks
        for i in range(5):
            task = await client.create_task(
                queue_name="emails",
                task_name="send-welcome",
                input={
                    "to": f"user{i}@example.com",
                    "subject": "Welcome to Valka!",
                },
                priority=i,
                max_retries=3,
            )
            print(f"Created task {task['id']} (status={task['status']})")

        # List all email tasks
        tasks = await client.list_tasks(queue_name="emails", limit=10)
        print(f"\nFound {len(tasks)} email tasks")

        # Check workers
        workers = await client.list_workers()
        print(f"Connected workers: {len(workers)}")
        for w in workers:
            print(f"  - {w['name']} (queues={w['queues']}, active={w['active_tasks']})")


if __name__ == "__main__":
    asyncio.run(main())
