// Step checkpoints: a three-step pipeline whose "transform" step fails on the first attempt.
// The retry skips the checkpointed "fetch" step and resumes at "transform".
//
// Usage: go run ./steps (requires a Valka server at localhost:50051). Enqueue work with:
//
//	curl -X POST localhost:8989/api/v1/tasks -H 'content-type: application/json' \
//	  -d '{"queue_name":"pipeline","task_name":"etl"}'
package main

import (
	"context"
	"errors"
	"fmt"
	"log"

	valka "github.com/valka-queue/valka/sdks/go"
)

func main() {
	worker, err := valka.NewWorker(
		valka.WithName("steps-worker"),
		valka.WithServerAddr("localhost:50051"),
		valka.WithQueues("pipeline"),
		valka.WithHandler(handleTask),
	)
	if err != nil {
		log.Fatalf("Failed to create worker: %v", err)
	}
	if err := worker.Run(context.Background()); err != nil {
		log.Fatalf("Worker error: %v", err)
	}
}

func handleTask(ctx *valka.TaskContext) (interface{}, error) {
	attempt := ctx.AttemptNumber
	rows, err := valka.Step(ctx, "fetch", func() (int, error) {
		log.Printf("fetch runs on attempt %d", attempt)
		return 3, nil
	})
	if err != nil {
		return nil, err
	}
	doubled, err := valka.Step(ctx, "transform", func() (int, error) {
		if attempt == 1 {
			return 0, errors.New("transform failed; the retry resumes here")
		}
		return rows * 2, nil
	})
	if err != nil {
		return nil, err
	}
	stored, err := valka.Step(ctx, "store", func() (string, error) {
		return fmt.Sprintf("stored %d rows", doubled), nil
	})
	if err != nil {
		return nil, err
	}
	return map[string]interface{}{"rows": rows, "doubled": doubled, "stored": stored, "attempt": attempt}, nil
}
