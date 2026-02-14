import { ValkaClient } from "@valka/sdk";

async function main() {
  const client = new ValkaClient("http://localhost:8989");
  console.log("Connected to Valka server");

  // Create a task with JSON input
  const task = await client.createTask({
    queue_name: "emails",
    task_name: "send-welcome",
    input: {
      to: "user@example.com",
      subject: "Welcome!",
      body: "Thanks for signing up.",
    },
  });

  console.log(`Created task: ${task.id}`);
  console.log(`  Queue:  ${task.queue_name}`);
  console.log(`  Status: ${task.status}`);

  // Retrieve the task by ID
  const fetched = await client.getTask(task.id);
  console.log(`\nFetched task: ${fetched.id} (status=${fetched.status})`);

  // List tasks in the "emails" queue
  const tasks = await client.listTasks({ queue_name: "emails", limit: 10 });
  console.log(`\nTasks in 'emails' queue: ${tasks.length}`);
  for (const t of tasks) {
    console.log(`  ${t.id} - ${t.task_name} (status=${t.status})`);
  }
}

main().catch(console.error);
