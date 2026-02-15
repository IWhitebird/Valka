"""Signal demo: creates a task with initial values, worker prints them in a loop,
producer sends update signals every 2s for ~15s, then sends a stop signal.

Run: python signal_demo.py
"""

import asyncio

from valka import ValkaClient, ValkaWorker, TaskContext


async def handle_task(ctx: TaskContext) -> dict:
    data = ctx.input()
    a = data["a"]
    b = data["b"]

    await ctx.log(f"Started with a={a}, b={b}")

    # Listen for signals in background
    stopped = asyncio.Event()

    async def signal_loop():
        nonlocal a, b
        while not stopped.is_set():
            sig = await ctx.receive_signal()
            if sig.name == "update":
                payload = sig.parse_payload()
                a = payload["a"]
                b = payload["b"]
                await ctx.log(f"Updated: a={a}, b={b}")
            elif sig.name == "stop":
                await ctx.log("Received stop signal")
                stopped.set()

    asyncio.create_task(signal_loop())

    # Print a + b every second until stopped
    while not stopped.is_set():
        await ctx.log(f"a={a}  b={b}  sum={a + b}")
        try:
            await asyncio.wait_for(stopped.wait(), timeout=1.0)
        except asyncio.TimeoutError:
            pass

    await ctx.log(f"Final: a={a}  b={b}  sum={a + b}")
    return {"status": "stopped gracefully"}


async def main() -> None:
    client = ValkaClient("http://localhost:8989")

    # Create task with initial a=4, b=5
    task = await client.create_task(
        queue_name="experiments",
        task_name="signal-demo",
        input={"a": 4, "b": 5},
    )
    print(f"Created task {task['id']}")

    # Start worker in background
    worker = (
        ValkaWorker.builder()
        .name("signal-demo-worker")
        .server_addr("localhost:50051")
        .queues(["experiments"])
        .handler(handle_task)
        .build()
    )
    worker_task = asyncio.create_task(worker.run())

    # Give worker time to connect and pick up the task
    await asyncio.sleep(3)

    # Send update signals every 2 seconds for ~14 seconds
    a, b = 4, 5
    for _ in range(7):
        await asyncio.sleep(2)
        a += 1
        b += 1
        resp = await client.send_signal(task["id"], "update", {"a": a, "b": b})
        print(f"[producer] Sent update a={a} b={b} (delivered={resp['delivered']})")

    # Send stop signal
    await asyncio.sleep(2)
    resp = await client.send_signal(task["id"], "stop")
    print(f"[producer] Sent stop signal (delivered={resp['delivered']})")

    # Wait for worker to finish
    await asyncio.sleep(2)
    print("Done.")

    worker_task.cancel()
    await client.close()


if __name__ == "__main__":
    asyncio.run(main())
