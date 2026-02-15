// Signal demo: creates a task with initial values, worker prints them in a loop,
// producer sends update signals every 2s for ~15s, then sends a stop signal.
package main

import (
	"context"
	"fmt"
	"log"
	"sync/atomic"
	"time"

	valka "github.com/valka-queue/valka/sdks/go"
)

func main() {
	client := valka.NewClient("http://localhost:8989")

	// Create task with initial a=4, b=5
	task, err := client.CreateTask(valka.CreateTaskRequest{
		QueueName: "experiments",
		TaskName:  "signal-demo",
		Input: map[string]interface{}{
			"a": 4,
			"b": 5,
		},
	})
	if err != nil {
		log.Fatalf("Failed to create task: %v", err)
	}
	fmt.Printf("Created task %s\n", task.ID)

	// Start worker in background
	go func() {
		worker, err := valka.NewWorker(
			valka.WithName("signal-demo-worker"),
			valka.WithServerAddr("localhost:50051"),
			valka.WithQueues("experiments"),
			valka.WithHandler(handleTask),
		)
		if err != nil {
			log.Fatalf("Failed to create worker: %v", err)
		}
		log.Println("Starting worker...")
		if err := worker.Run(context.Background()); err != nil {
			log.Fatalf("Worker error: %v", err)
		}
	}()

	// Give worker time to connect and pick up the task
	time.Sleep(3 * time.Second)

	// Send update signals every 2 seconds for ~14 seconds, incrementing a and b by 1
	a, b := 4, 5
	for i := 0; i < 7; i++ {
		time.Sleep(2 * time.Second)
		a++
		b++
		resp, err := client.SendSignal(task.ID, "update", map[string]interface{}{
			"a": a,
			"b": b,
		})
		if err != nil {
			log.Printf("Failed to send update signal: %v", err)
		} else {
			fmt.Printf("[producer] Sent update a=%d b=%d (delivered=%v)\n", a, b, resp.Delivered)
		}
	}

	// Send stop signal
	time.Sleep(2 * time.Second)
	resp, err := client.SendSignal(task.ID, "stop", nil)
	if err != nil {
		log.Printf("Failed to send stop signal: %v", err)
	} else {
		fmt.Printf("[producer] Sent stop signal (delivered=%v)\n", resp.Delivered)
	}

	// Wait for worker to finish
	time.Sleep(2 * time.Second)
	fmt.Println("Done.")
}

func handleTask(ctx *valka.TaskContext) (interface{}, error) {
	var input map[string]interface{}
	if err := ctx.Input(&input); err != nil {
		return nil, err
	}

	var a, b atomic.Int64
	a.Store(int64(input["a"].(float64)))
	b.Store(int64(input["b"].(float64)))

	ctx.Log(fmt.Sprintf("Started with a=%d, b=%d", a.Load(), b.Load()))

	// Listen for signals in background
	stopCh := make(chan struct{})
	go func() {
		for {
			sig, err := ctx.ReceiveSignal()
			if err != nil {
				close(stopCh)
				return
			}
			switch sig.Name {
			case "update":
				var payload map[string]interface{}
				if err := sig.ParsePayload(&payload); err != nil {
					ctx.Warn(fmt.Sprintf("Bad update payload: %v", err))
					continue
				}
				a.Store(int64(payload["a"].(float64)))
				b.Store(int64(payload["b"].(float64)))
				ctx.Log(fmt.Sprintf("Updated: a=%d, b=%d", a.Load(), b.Load()))
			case "stop":
				ctx.Log("Received stop signal")
				close(stopCh)
				return
			}
		}
	}()

	// Print a + b every second until stopped
	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ticker.C:
			av, bv := a.Load(), b.Load()
			ctx.Log(fmt.Sprintf("a=%d  b=%d  sum=%d", av, bv, av+bv))
		case <-stopCh:
			av, bv := a.Load(), b.Load()
			ctx.Log(fmt.Sprintf("Final: a=%d  b=%d  sum=%d", av, bv, av+bv))
			return map[string]string{"status": "stopped gracefully"}, nil
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
}
