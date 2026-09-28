// Step checkpoints: a three-step pipeline whose `transform` step fails on the first attempt.
// The retry skips the checkpointed `fetch` step and resumes at `transform`.
//
// Usage: npm run steps (requires a Valka server at localhost:50051). Enqueue work with:
//   curl -X POST localhost:8989/api/v1/tasks -H 'content-type: application/json' \
//     -d '{"queue_name":"pipeline","task_name":"etl"}'

import { ValkaWorker, type TaskContext } from "@valka/sdk";

async function handleTask(ctx: TaskContext): Promise<unknown> {
  const attempt = ctx.attemptNumber;
  const rows = await ctx.step("fetch", () => {
    console.log(`fetch runs on attempt ${attempt}`);
    return 3;
  });
  const doubled = await ctx.step("transform", () => {
    if (attempt === 1) {
      throw new Error("transform failed; the retry resumes here");
    }
    return rows * 2;
  });
  const stored = await ctx.step("store", () => `stored ${doubled} rows`);
  return { rows, doubled, stored, attempt };
}

const worker = ValkaWorker.builder()
  .name("steps-worker")
  .serverAddr("localhost:50051")
  .queues(["pipeline"])
  .handler(handleTask)
  .build();

worker.run().catch(console.error);
