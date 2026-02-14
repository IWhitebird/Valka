package main

import (
	"fmt"
	"log"

	valka "github.com/valka-queue/valka/sdks/go"
)

func main() {
	client := valka.NewClient("http://localhost:8080")

	// Health check
	health, err := client.HealthCheck()
	if err != nil {
		log.Fatalf("Health check failed: %v", err)
	}
	fmt.Printf("Server health: %s\n", health)

	// Create several tasks
	for i := 0; i < 5; i++ {
		priority := i
		task, err := client.CreateTask(valka.CreateTaskRequest{
			QueueName: "emails",
			TaskName:  "send-welcome",
			Input: map[string]interface{}{
				"to":      fmt.Sprintf("user%d@example.com", i),
				"subject": "Welcome to Valka!",
			},
			Priority:   &priority,
			MaxRetries: intPtr(3),
		})
		if err != nil {
			log.Fatalf("Failed to create task: %v", err)
		}
		fmt.Printf("Created task %s (status=%s)\n", task.ID, task.Status)
	}

	// List all email tasks
	tasks, err := client.ListTasks(valka.ListTasksParams{
		QueueName: "emails",
		Limit:     10,
	})
	if err != nil {
		log.Fatalf("Failed to list tasks: %v", err)
	}
	fmt.Printf("\nFound %d email tasks\n", len(tasks))

	// Check workers
	workers, err := client.ListWorkers()
	if err != nil {
		log.Fatalf("Failed to list workers: %v", err)
	}
	fmt.Printf("Connected workers: %d\n", len(workers))
	for _, w := range workers {
		fmt.Printf("  - %s (queues=%v, active=%d)\n", w.Name, w.Queues, w.ActiveTasks)
	}
}

func intPtr(n int) *int { return &n }
