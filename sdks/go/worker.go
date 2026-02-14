package valka

import (
	"context"
	"crypto/rand"
	"encoding/json"
	"fmt"
	"log"
	"os"
	"os/signal"
	"sync"
	"syscall"
	"time"

	pb "github.com/valka-queue/valka/sdks/go/proto/valkav1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
	"google.golang.org/grpc/keepalive"
)

// TaskHandler is the function signature for processing tasks.
type TaskHandler func(ctx *TaskContext) (interface{}, error)

// ValkaWorker connects to the Valka server via gRPC bidirectional streaming.
type ValkaWorker struct {
	workerID    string
	name        string
	serverAddr  string
	queues      []string
	concurrency int
	metadata    map[string]interface{}
	handler     TaskHandler

	semaphore    chan struct{}
	activeTasks  sync.Map
	wg           sync.WaitGroup
	shuttingDown bool
	shutdownMu   sync.Mutex
	shutdownCh   chan struct{}
	sendCh       chan *pb.WorkerRequest
}

// NewWorker creates a new ValkaWorker with functional options.
func NewWorker(opts ...WorkerOption) (*ValkaWorker, error) {
	cfg := defaultConfig()
	for _, opt := range opts {
		if err := opt(cfg); err != nil {
			return nil, err
		}
	}
	if err := cfg.validate(); err != nil {
		return nil, err
	}

	workerID := generateUUID()
	name := cfg.name
	if name == "" {
		name = fmt.Sprintf("go-worker-%s", workerID[:8])
	}

	return &ValkaWorker{
		workerID:    workerID,
		name:        name,
		serverAddr:  cfg.serverAddr,
		queues:      cfg.queues,
		concurrency: cfg.concurrency,
		metadata:    cfg.metadata,
		handler:     cfg.handler,
		semaphore:   make(chan struct{}, cfg.concurrency),
		shutdownCh:  make(chan struct{}),
		sendCh:      make(chan *pb.WorkerRequest, 256),
	}, nil
}

// Run starts the worker. Blocks until shutdown or unrecoverable error.
func (w *ValkaWorker) Run(ctx context.Context) error {
	// Handle OS signals
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		select {
		case <-sigCh:
			w.Shutdown()
		case <-w.shutdownCh:
		}
	}()

	retry := DefaultRetryPolicy()
	for {
		w.shutdownMu.Lock()
		done := w.shuttingDown
		w.shutdownMu.Unlock()
		if done {
			return nil
		}

		err := w.session(ctx)
		if err == nil {
			return nil
		}

		w.shutdownMu.Lock()
		done = w.shuttingDown
		w.shutdownMu.Unlock()
		if done {
			return nil
		}

		delay := retry.NextDelay()
		log.Printf("[valka] Connection lost (%v), reconnecting in %v", err, delay)
		select {
		case <-time.After(delay):
		case <-w.shutdownCh:
			return nil
		case <-ctx.Done():
			return ctx.Err()
		}
	}
}

// Shutdown initiates graceful shutdown.
func (w *ValkaWorker) Shutdown() {
	w.shutdownMu.Lock()
	if w.shuttingDown {
		w.shutdownMu.Unlock()
		return
	}
	w.shuttingDown = true
	w.shutdownMu.Unlock()

	log.Println("[valka] Shutting down, draining active tasks...")

	// Send graceful shutdown message
	w.sendCh <- &pb.WorkerRequest{
		Request: &pb.WorkerRequest_Shutdown{
			Shutdown: &pb.GracefulShutdown{
				Reason: "client shutdown",
			},
		},
	}

	// Wait up to 30s for active tasks to drain
	done := make(chan struct{})
	go func() {
		w.wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		log.Println("[valka] All tasks drained")
	case <-time.After(30 * time.Second):
		log.Println("[valka] Drain timeout, forcing shutdown")
	}

	close(w.shutdownCh)
}

func (w *ValkaWorker) session(ctx context.Context) error {
	conn, err := grpc.NewClient(
		w.serverAddr,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithKeepaliveParams(keepalive.ClientParameters{
			Time:    10 * time.Second,
			Timeout: 5 * time.Second,
		}),
	)
	if err != nil {
		return NewConnectionError("failed to connect", err)
	}
	defer conn.Close()

	client := pb.NewWorkerServiceClient(conn)
	stream, err := client.Session(ctx)
	if err != nil {
		return NewConnectionError("failed to open session", err)
	}

	// Send hello
	metaStr := ""
	if w.metadata != nil {
		data, _ := json.Marshal(w.metadata)
		metaStr = string(data)
	}
	hello := &pb.WorkerRequest{
		Request: &pb.WorkerRequest_Hello{
			Hello: &pb.WorkerHello{
				WorkerId:    w.workerID,
				WorkerName:  w.name,
				Queues:      w.queues,
				Concurrency: int32(w.concurrency),
				Metadata:    metaStr,
			},
		},
	}
	if err := stream.Send(hello); err != nil {
		return NewConnectionError("failed to send hello", err)
	}

	log.Printf("[valka] Connected as %s (id=%s, queues=%v, concurrency=%d)",
		w.name, w.workerID, w.queues, w.concurrency)

	// Dedicated sender goroutine — stream.Send is not thread-safe
	senderCtx, senderCancel := context.WithCancel(ctx)
	defer senderCancel()

	go w.senderLoop(senderCtx, stream)
	go w.heartbeatLoop(senderCtx)

	// Receive loop
	for {
		resp, err := stream.Recv()
		if err != nil {
			return NewConnectionError("stream receive error", err)
		}

		w.shutdownMu.Lock()
		done := w.shuttingDown
		w.shutdownMu.Unlock()
		if done {
			return nil
		}

		w.handleResponse(ctx, resp)
	}
}

func (w *ValkaWorker) senderLoop(ctx context.Context, stream pb.WorkerService_SessionClient) {
	for {
		select {
		case <-ctx.Done():
			return
		case msg, ok := <-w.sendCh:
			if !ok {
				return
			}
			if err := stream.Send(msg); err != nil {
				log.Printf("[valka] Send error: %v", err)
				return
			}
		}
	}
}

func (w *ValkaWorker) heartbeatLoop(ctx context.Context) {
	ticker := time.NewTicker(10 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-w.shutdownCh:
			return
		case <-ticker.C:
			var activeIDs []string
			w.activeTasks.Range(func(key, _ interface{}) bool {
				activeIDs = append(activeIDs, key.(string))
				return true
			})

			w.sendCh <- &pb.WorkerRequest{
				Request: &pb.WorkerRequest_Heartbeat{
					Heartbeat: &pb.Heartbeat{
						ActiveTaskIds: activeIDs,
						TimestampMs:   time.Now().UnixMilli(),
					},
				},
			}
		}
	}
}

func (w *ValkaWorker) handleResponse(ctx context.Context, resp *pb.WorkerResponse) {
	switch msg := resp.Response.(type) {
	case *pb.WorkerResponse_TaskAssignment:
		w.handleTaskAssignment(ctx, msg.TaskAssignment)
	case *pb.WorkerResponse_TaskCancellation:
		w.handleTaskCancellation(msg.TaskCancellation)
	case *pb.WorkerResponse_ServerShutdown:
		log.Printf("[valka] Server shutdown: %s", msg.ServerShutdown.Reason)
		go w.Shutdown()
	case *pb.WorkerResponse_HeartbeatAck:
		// no-op
	}
}

func (w *ValkaWorker) handleTaskAssignment(ctx context.Context, assignment *pb.TaskAssignment) {
	// Acquire semaphore slot
	w.semaphore <- struct{}{}
	w.wg.Add(1)

	taskCtx, taskCancel := context.WithCancel(ctx)
	w.activeTasks.Store(assignment.TaskId, taskCancel)

	go func() {
		defer func() {
			<-w.semaphore
			w.activeTasks.Delete(assignment.TaskId)
			w.wg.Done()
		}()

		w.executeTask(taskCtx, taskCancel, assignment)
	}()
}

func (w *ValkaWorker) handleTaskCancellation(cancellation *pb.TaskCancellation) {
	if cancelFn, ok := w.activeTasks.Load(cancellation.TaskId); ok {
		log.Printf("[valka] Task %s cancelled: %s", cancellation.TaskId, cancellation.Reason)
		cancelFn.(context.CancelFunc)()
	}
}

func (w *ValkaWorker) executeTask(ctx context.Context, cancel context.CancelFunc, assignment *pb.TaskAssignment) {
	defer cancel()

	tctx := &TaskContext{
		Context:       ctx,
		TaskID:        assignment.TaskId,
		TaskRunID:     assignment.TaskRunId,
		QueueName:     assignment.QueueName,
		TaskName:      assignment.TaskName,
		AttemptNumber: assignment.AttemptNumber,
		RawInput:      assignment.Input,
		RawMetadata:   assignment.Metadata,
		sendFn: func(req *pb.WorkerRequest) {
			select {
			case w.sendCh <- req:
			default:
				// Drop if channel is full to avoid blocking
			}
		},
		cancel: cancel,
	}

	success := false
	retryable := true
	output := ""
	errorMessage := ""

	result, err := w.handler(tctx)
	if err != nil {
		if he, ok := err.(*HandlerError); ok {
			retryable = he.Retryable
			errorMessage = he.Error()
		} else {
			errorMessage = err.Error()
		}
		log.Printf("[valka] Task %s failed: %s", assignment.TaskId, errorMessage)
	} else {
		success = true
		if result != nil {
			data, err := json.Marshal(result)
			if err != nil {
				output = fmt.Sprintf("%v", result)
			} else {
				output = string(data)
			}
		}
	}

	w.sendCh <- &pb.WorkerRequest{
		Request: &pb.WorkerRequest_TaskResult{
			TaskResult: &pb.TaskResult{
				TaskId:       assignment.TaskId,
				TaskRunId:    assignment.TaskRunId,
				Success:      success,
				Retryable:    retryable,
				Output:       output,
				ErrorMessage: errorMessage,
			},
		},
	}
}

// generateUUID generates a UUID v4 using crypto/rand (no external deps).
func generateUUID() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	b[6] = (b[6] & 0x0f) | 0x40 // version 4
	b[8] = (b[8] & 0x3f) | 0x80 // variant 2
	return fmt.Sprintf("%08x-%04x-%04x-%04x-%012x",
		b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}
