package main

import (
	"context"
	"fmt"
	"log"
	"time"

	valka "github.com/valka-queue/valka/sdks/go"
)

func main() {
	worker, err := valka.NewWorker(
		valka.WithName("go-email-worker"),
		valka.WithServerAddr("localhost:50051"),
		valka.WithQueues("emails", "notifications"),
		valka.WithConcurrency(4),
		valka.WithHandler(handleTask),
	)
	if err != nil {
		log.Fatalf("Failed to create worker: %v", err)
	}

	log.Println("Starting worker...")
	if err := worker.Run(context.Background()); err != nil {
		log.Fatalf("Worker error: %v", err)
	}
}

func handleTask(ctx *valka.TaskContext) (interface{}, error) {
	var input map[string]interface{}
	if err := ctx.Input(&input); err != nil {
		return nil, err
	}

	to := "unknown"
	if v, ok := input["to"].(string); ok {
		to = v
	}

	ctx.Log(fmt.Sprintf("Processing email task: %s", ctx.TaskName))
	ctx.Debug(fmt.Sprintf("Attempt #%d", ctx.AttemptNumber))

	// Simulate work
	time.Sleep(1 * time.Second)

	ctx.Log(fmt.Sprintf("Email sent to %s", to))

	return map[string]interface{}{
		"delivered_to": to,
		"status":       "sent",
	}, nil
}
