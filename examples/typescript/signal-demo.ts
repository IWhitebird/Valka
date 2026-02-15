// Signal demo: creates a task with initial values, worker prints them in a loop,
// producer sends update signals every 2s for ~15s, then sends a stop signal.
//
// Run: npx tsx signal-demo.ts

import { ValkaClient, ValkaWorker, TaskContext } from "@valka/sdk";

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function handleTask(ctx: TaskContext): Promise<unknown> {
  const input = ctx.input<{ a: number; b: number }>();
  let a = input.a;
  let b = input.b;

  ctx.log(`Started with a=${a}, b=${b}`);
  
  // Listen for signals in background
  let stopped = false;
  const signalLoop = (async () => {
    while (!stopped) {
      const sig = await ctx.receiveSignal();
      if (sig.name === "update") {
        const payload = TaskContext.parseSignalPayload<{ a: number; b: number }>(sig);
        a = payload.a;
        b = payload.b;
        ctx.log(`Updated: a=${a}, b=${b}`);
      } else if (sig.name === "stop") {
        ctx.log("Received stop signal");
        stopped = true;
      }
    }
  })();

  // Print a + b every second until stopped
  while (!stopped) {
    ctx.log(`a=${a}  b=${b}  sum=${a + b}`);
    await sleep(1000);
  }

  ctx.log(`Final: a=${a}  b=${b}  sum=${a + b}`);
  return { status: "stopped gracefully" };
}

async function main() {
  const client = new ValkaClient("http://localhost:8989");

  // Create task with initial a=4, b=5
  const task = await client.createTask({
    queue_name: "experiments",
    task_name: "signal-demo",
    input: { a: 4, b: 5 },
  });
  console.log(`Created task ${task.id}`);

  // Start worker in background
  const worker = ValkaWorker.builder()
    .name("signal-demo-worker")
    .serverAddr("localhost:50051")
    .queues(["experiments"])
    .handler(handleTask)
    .build();

  const workerPromise = worker.run();

  // Give worker time to connect and pick up the task
  await sleep(3000);

  // Send update signals every 2 seconds for ~14 seconds, incrementing a and b by 1
  let a = 4;
  let b = 5;
  for (let i = 0; i < 7; i++) {
    await sleep(2000);
    a++;
    b++;
    const resp = await client.sendSignal(task.id, "update", { a, b });
    console.log(`[producer] Sent update a=${a} b=${b} (delivered=${resp.delivered})`);
  }

  // Send stop signal
  await sleep(2000);
  const resp = await client.sendSignal(task.id, "stop");
  console.log(`[producer] Sent stop signal (delivered=${resp.delivered})`);

  // Wait a bit then exit
  await sleep(2000);
  console.log("Done.");
  process.exit(0);
}

main().catch(console.error);
