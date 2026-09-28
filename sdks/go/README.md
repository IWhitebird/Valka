# Valka Go SDK

Go SDK for the [Valka](https://github.com/your-org/valka) distributed task queue.

## Installation

```bash
go get github.com/valka-queue/valka/sdks/go
```

## Quick Start

### Client — Create and manage tasks (REST)

```go
package main

import (
    "fmt"
    "log"

    valka "github.com/valka-queue/valka/sdks/go"
)

func main() {
    client := valka.NewClient("http://localhost:8989")

    // Create a task
    task, err := client.CreateTask(valka.CreateTaskRequest{
        QueueName: "emails",
        TaskName:  "send-welcome",
        Input:     map[string]string{"to": "user@example.com"},
    })
    if err != nil {
        log.Fatal(err)
    }
    fmt.Printf("Created task %s\n", task.ID)

    // Get task status
    task, err = client.GetTask(task.ID)
    if err != nil {
        log.Fatal(err)
    }
    fmt.Printf("Status: %s\n", task.Status)
}
```

### Worker — Process tasks (gRPC)

```go
package main

import (
    "context"
    "log"

    valka "github.com/valka-queue/valka/sdks/go"
)

func main() {
    worker, err := valka.NewWorker(
        valka.WithName("email-worker"),
        valka.WithServerAddr("localhost:50051"),
        valka.WithQueues("emails"),
        valka.WithConcurrency(4),
        valka.WithHandler(func(ctx *valka.TaskContext) (interface{}, error) {
            var input map[string]interface{}
            ctx.Input(&input)
            ctx.Log(fmt.Sprintf("Sending email to %v", input["to"]))
            return map[string]bool{"delivered": true}, nil
        }),
    )
    if err != nil {
        log.Fatal(err)
    }
    if err := worker.Run(context.Background()); err != nil {
        log.Fatal(err)
    }
}
```

### Step checkpoints

`valka.Step` runs a named step once per task: its result is checkpointed on the server, and
a retry of the task returns the recorded result instead of running the step again.

```go
func handle(ctx *valka.TaskContext) (interface{}, error) {
    rows, err := valka.Step(ctx, "fetch", func() ([]Row, error) { return fetchRows(ctx) })
    if err != nil {
        return nil, err
    }
    return valka.Step(ctx, "upload", func() (string, error) { return upload(ctx, rows) })
}
```

`ctx.Checkpoint(step, value)` and `ctx.CheckpointValue(step, &dest)` record and read
checkpoints directly. Step results must round-trip through `encoding/json`.

## Requirements

- Go 1.21+
- A running Valka server

## Proto Generation

Proto stubs are pre-generated. To regenerate:

```bash
# Install protoc plugins
go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest

# Generate
bash generate_proto.sh
# Or: go generate ./...
```
