import { ValkaWorker, type TaskContext } from "@valka/sdk";

async function handleTask(ctx: TaskContext): Promise<unknown> {
  ctx.log(`Processing task '${ctx.taskName}' (attempt ${ctx.attemptNumber})`);

  const input = ctx.input<{ to?: string; subject?: string; body?: string }>();
  const to = input.to ?? "unknown";

  ctx.log(`Sending email to ${to}...`);
  await sleep(1000);
  ctx.log("Email sent successfully");

  return { delivered_to: to, status: "sent" };
}

async function main() {
  const worker = ValkaWorker.builder()
    .name("example-worker")
    .serverAddr("localhost:50051")
    .queues(["emails"])
    .concurrency(4)
    .handler(handleTask)
    .build();

  console.log("Worker starting... press Ctrl+C to stop.");
  await worker.run();
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch(console.error);
