import { ValkaClient, ValkaWorker, type TaskContext } from "@valka/sdk";

async function main() {
  const restAddr = "http://localhost:8989";
  const grpcAddr = "localhost:50051";

  // Start the worker in the background
  const worker = ValkaWorker.builder()
    .name("lifecycle-worker")
    .serverAddr(grpcAddr)
    .queues(["demo"])
    .concurrency(2)
    .handler(async (ctx: TaskContext) => {
      ctx.log(`Handling: ${ctx.taskName}`);
      const input = ctx.input<{ n?: number }>();
      const n = input.n ?? 0;
      const result = n * 2;
      ctx.log(`Computed ${n} * 2 = ${result}`);
      return { result };
    })
    .build();

  // Run worker in background (don't await)
  const workerPromise = worker.run();
  void workerPromise;

  // Give the worker time to connect
  await sleep(2000);

  // Create some tasks via REST
  const client = new ValkaClient(restAddr);

  console.log("Creating 5 tasks...");
  const taskIds: string[] = [];
  for (let i = 1; i <= 5; i++) {
    const task = await client.createTask({
      queue_name: "demo",
      task_name: "multiply",
      input: { n: i },
    });
    console.log(`  Created task ${task.id} with n=${i}`);
    taskIds.push(task.id);
  }

  // Poll until all tasks complete (or timeout)
  console.log("\nWaiting for tasks to complete...");
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    await sleep(1000);
    let completed = 0;
    for (const id of taskIds) {
      const t = await client.getTask(id);
      if (t.status === "COMPLETED" || t.status === "FAILED") completed++;
    }
    console.log(`  ${completed}/${taskIds.length} tasks completed`);
    if (completed >= taskIds.length) break;
  }

  // Print final status
  console.log("\nFinal task statuses:");
  for (const id of taskIds) {
    const task = await client.getTask(id);
    console.log(`  ${task.id} - status=${task.status} output=${JSON.stringify(task.output)}`);
  }

  console.log("\nShutting down worker...");
  worker.shutdown();
  await workerPromise.catch(() => {});
  console.log("Done!");
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch(console.error);
