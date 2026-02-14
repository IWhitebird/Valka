// Package valka provides a Go SDK for the Valka distributed task queue.
//
// The SDK has two components:
//
//   - [ValkaClient] — REST client for task CRUD operations (create, get, list, cancel).
//   - [ValkaWorker] — gRPC bidirectional streaming worker for processing tasks.
//
// # Client Usage
//
//	client := valka.NewClient("http://localhost:8080")
//	task, err := client.CreateTask(valka.CreateTaskRequest{
//	    QueueName: "emails",
//	    TaskName:  "send-welcome",
//	    Input:     map[string]string{"to": "user@example.com"},
//	})
//
// # Worker Usage
//
//	worker, err := valka.NewWorker(
//	    valka.WithName("email-worker"),
//	    valka.WithServerAddr("localhost:50051"),
//	    valka.WithQueues("emails"),
//	    valka.WithConcurrency(4),
//	    valka.WithHandler(func(ctx *valka.TaskContext) (interface{}, error) {
//	        ctx.Log("Processing task...")
//	        return map[string]bool{"done": true}, nil
//	    }),
//	)
//	if err != nil {
//	    log.Fatal(err)
//	}
//	worker.Run(context.Background())
package valka
