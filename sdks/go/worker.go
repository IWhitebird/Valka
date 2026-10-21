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
	"sync/atomic"
	"syscall"
	"time"

	pb "github.com/IWhitebird/Valka/sdks/go/proto/valkav1"
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
	serverAddrs []string
	nextAddr    atomic.Uint64
	apiKey      string
	queues      []string
	concurrency int
	metadata    map[string]interface{}
	handler     TaskHandler

	semaphore chan struct{}
	// Per-task maps hold the latest run of each task. An older run of the same task still
	// finishing here is stale (the server starts a new run only after the old one ended),
	// so its cleanup compares before deleting and never removes the newer run.
	activeTasks    sync.Map // task_id -> *runState
	runs           sync.Map // task_id -> *pb.RunningTask, sent in WorkerHello on every connect
	signalChannels sync.Map // task_id -> chan *pb.TaskSignal
	unacked        sync.Map // task_run_id -> *pb.TaskResult, kept until APPLIED or STALE
	clientMu       sync.RWMutex
	client         pb.WorkerServiceClient // current session's client, nil between sessions
	executing      atomic.Int64           // handlers running or waiting for a slot
	shuttingDown   bool
	shutdownMu     sync.Mutex
	drainCh        chan struct{} // closed when a graceful shutdown starts
	shutdownCh     chan struct{} // closed when the drain is over
	sendCh         chan *pb.WorkerRequest
}

// runState is one run executing on this worker.
type runState struct {
	runID  string
	cancel context.CancelFunc
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
		serverAddrs: cfg.serverAddrs,
		apiKey:      cfg.apiKey,
		queues:      cfg.queues,
		concurrency: cfg.concurrency,
		metadata:    cfg.metadata,
		handler:     cfg.handler,
		semaphore:   make(chan struct{}, cfg.concurrency),
		drainCh:     make(chan struct{}),
		shutdownCh:  make(chan struct{}),
		sendCh:      make(chan *pb.WorkerRequest, 256),
	}, nil
}

// Run starts the worker. Blocks until shutdown or unrecoverable error.
//
// Shutdown (or SIGINT/SIGTERM) drains: the server stops sending tasks, running handlers
// finish and their results are acknowledged, then Run returns. A connection lost meanwhile
// is re-established (on the next node) and the drain goes on there.
func (w *ValkaWorker) Run(ctx context.Context) error {
	// Handle OS signals
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	defer signal.Stop(sigCh)
	runDone := make(chan struct{})
	defer close(runDone)
	go func() {
		select {
		case <-sigCh:
			w.Shutdown()
		case <-w.shutdownCh:
		case <-runDone:
		}
	}()

	retry := DefaultRetryPolicy()
	for {
		if w.drained() {
			return nil
		}

		err := w.session(ctx, retry)
		if w.drained() {
			return nil
		}

		// Next time, try the next node: any node takes any worker.
		w.nextAddr.Add(1)
		delay := retry.NextDelay()
		if w.draining() && delay > drainReconnectDelay {
			delay = drainReconnectDelay
		}
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

// Shutdown initiates graceful shutdown and blocks until the drain is over: the server is
// told to send no more tasks (on the current connection and any later one), running
// handlers get up to 30s to finish, and their results up to 10s more to be acknowledged.
func (w *ValkaWorker) Shutdown() {
	w.shutdownMu.Lock()
	if w.shuttingDown {
		w.shutdownMu.Unlock()
		return
	}
	w.shuttingDown = true
	// Never blocks: the session's sender sends the GracefulShutdown.
	close(w.drainCh)
	w.shutdownMu.Unlock()

	log.Println("[valka] Shutting down, draining active tasks...")

	deadline := time.Now().Add(drainTimeout)
	for w.executing.Load() > 0 && time.Now().Before(deadline) {
		time.Sleep(drainPoll)
	}
	if w.executing.Load() > 0 {
		log.Println("[valka] Drain timeout, forcing shutdown")
	} else {
		log.Println("[valka] All tasks drained")
	}

	ackDeadline := time.Now().Add(shutdownAckWait)
	for w.unackedCount() > 0 && time.Now().Before(ackDeadline) {
		time.Sleep(drainPoll)
	}

	close(w.shutdownCh)
}

// draining reports whether a graceful shutdown has started.
func (w *ValkaWorker) draining() bool {
	select {
	case <-w.drainCh:
		return true
	default:
		return false
	}
}

// drained reports whether the graceful shutdown is over.
func (w *ValkaWorker) drained() bool {
	select {
	case <-w.shutdownCh:
		return true
	default:
		return false
	}
}

func shutdownRequest() *pb.WorkerRequest {
	return &pb.WorkerRequest{
		Request: &pb.WorkerRequest_Shutdown{
			Shutdown: &pb.GracefulShutdown{Reason: "client shutdown"},
		},
	}
}

const (
	resultRetryDelay    = time.Second
	drainTimeout        = 30 * time.Second
	shutdownAckWait     = 10 * time.Second
	drainPoll           = 50 * time.Millisecond
	drainReconnectDelay = time.Second
)

func resultRequest(result *pb.TaskResult) *pb.WorkerRequest {
	return &pb.WorkerRequest{Request: &pb.WorkerRequest_TaskResult{TaskResult: result}}
}

func (w *ValkaWorker) trySend(req *pb.WorkerRequest) {
	select {
	case w.sendCh <- req:
	default:
	}
}

// deliver keeps the result until the server answers APPLIED or STALE. Unacked results
// are resent when a session starts and on every heartbeat tick.
func (w *ValkaWorker) deliver(result *pb.TaskResult) {
	w.unacked.Store(result.TaskRunId, result)
	w.trySend(resultRequest(result))
}

func (w *ValkaWorker) resendUnacked() {
	w.unacked.Range(func(_, v interface{}) bool {
		w.trySend(resultRequest(v.(*pb.TaskResult)))
		return true
	})
}

func (w *ValkaWorker) unackedCount() int {
	n := 0
	w.unacked.Range(func(_, _ interface{}) bool {
		n++
		return true
	})
	return n
}

func (w *ValkaWorker) setClient(c pb.WorkerServiceClient) {
	w.clientMu.Lock()
	defer w.clientMu.Unlock()
	w.client = c
}

// checkpoint goes through the current session, so a task that outlives a reconnect can
// still checkpoint.
func (w *ValkaWorker) checkpoint(ctx context.Context, req *pb.CheckpointRequest) error {
	w.clientMu.RLock()
	c := w.client
	w.clientMu.RUnlock()
	if c == nil {
		return NewConnectionError("not connected to the server", nil)
	}
	_, err := c.Checkpoint(ctx, req)
	return err
}

func (w *ValkaWorker) handleResultAck(ack *pb.ResultAck) {
	switch ack.Status {
	case pb.ResultStatus_RESULT_STATUS_APPLIED, pb.ResultStatus_RESULT_STATUS_STALE:
		if ack.Status == pb.ResultStatus_RESULT_STATUS_STALE {
			log.Printf("[valka] Result for task %s not recorded: the run already ended another way", ack.TaskId)
		}
		w.unacked.Delete(ack.TaskRunId)
	default:
		time.AfterFunc(resultRetryDelay, func() {
			if r, ok := w.unacked.Load(ack.TaskRunId); ok {
				w.trySend(resultRequest(r.(*pb.TaskResult)))
			}
		})
	}
}

func (w *ValkaWorker) session(ctx context.Context, retry *RetryPolicy) error {
	addr := w.serverAddrs[w.nextAddr.Load()%uint64(len(w.serverAddrs))]
	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithKeepaliveParams(keepalive.ClientParameters{
			Time:    10 * time.Second,
			Timeout: 5 * time.Second,
		}),
	}
	if w.apiKey != "" {
		dialOpts = append(dialOpts, grpc.WithPerRPCCredentials(apiKeyCreds(w.apiKey)))
	}
	conn, err := grpc.NewClient(addr, dialOpts...)
	if err != nil {
		return NewConnectionError("failed to connect", err)
	}
	defer conn.Close()

	client := pb.NewWorkerServiceClient(conn)
	w.setClient(client)
	defer w.setClient(nil)
	streamCtx, closeStream := context.WithCancel(ctx)
	defer closeStream()
	go func() {
		select {
		case <-w.shutdownCh:
			closeStream()
		case <-streamCtx.Done():
		}
	}()
	stream, err := client.Session(streamCtx)
	if err != nil {
		return NewConnectionError("failed to open session", err)
	}

	if err := stream.Send(w.helloRequest()); err != nil {
		return NewConnectionError("failed to send hello", err)
	}
	// A drain carried over from an earlier connection: no new tasks on this one either.
	drainSent := false
	if w.draining() {
		if err := stream.Send(shutdownRequest()); err != nil {
			return NewConnectionError("failed to send shutdown", err)
		}
		drainSent = true
	}

	log.Printf("[valka] Connected as %s (id=%s, queues=%v, concurrency=%d)",
		w.name, w.workerID, w.queues, w.concurrency)

	// Dedicated sender goroutine — stream.Send is not thread-safe
	senderCtx, senderCancel := context.WithCancel(ctx)
	defer senderCancel()

	go w.senderLoop(senderCtx, stream, drainSent)
	go w.heartbeatLoop(senderCtx)
	w.resendUnacked()

	answered := false
	for {
		resp, err := stream.Recv()
		if err != nil {
			return NewConnectionError("stream receive error", err)
		}
		// The server took the session: back off from scratch after it ends.
		if !answered {
			answered = true
			retry.Reset()
		}
		if stopping := w.handleResponse(ctx, resp); stopping {
			// Reconnect to another node; a drain in progress goes on there.
			return NewConnectionError("server shutting down", nil)
		}
	}
}

func (w *ValkaWorker) senderLoop(ctx context.Context, stream pb.WorkerService_SessionClient, drainSent bool) {
	drain := w.drainCh
	if drainSent {
		drain = nil
	}
	for {
		select {
		case <-ctx.Done():
			return
		case <-drain:
			// A shutdown started during this connection: tell the server once.
			drain = nil
			if err := stream.Send(shutdownRequest()); err != nil {
				log.Printf("[valka] Send error: %v", err)
				return
			}
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
			w.unacked.Range(func(_, v interface{}) bool {
				activeIDs = append(activeIDs, v.(*pb.TaskResult).TaskId)
				return true
			})
			w.resendUnacked()

			hb := &pb.WorkerRequest{
				Request: &pb.WorkerRequest_Heartbeat{
					Heartbeat: &pb.Heartbeat{
						ActiveTaskIds: activeIDs,
						TimestampMs:   time.Now().UnixMilli(),
					},
				},
			}
			select {
			case w.sendCh <- hb:
			case <-ctx.Done():
				return
			case <-w.shutdownCh:
				return
			}
		}
	}
}

// handleResponse reports whether the server is shutting down (the worker then reconnects).
func (w *ValkaWorker) handleResponse(ctx context.Context, resp *pb.WorkerResponse) bool {
	switch msg := resp.Response.(type) {
	case *pb.WorkerResponse_TaskAssignment:
		w.handleTaskAssignment(ctx, msg.TaskAssignment)
	case *pb.WorkerResponse_TaskCancellation:
		w.handleTaskCancellation(msg.TaskCancellation)
	case *pb.WorkerResponse_TaskSignal:
		w.handleTaskSignal(msg.TaskSignal)
	case *pb.WorkerResponse_ResultAck:
		w.handleResultAck(msg.ResultAck)
	case *pb.WorkerResponse_ServerShutdown:
		log.Printf("[valka] Server shutdown: %s", msg.ServerShutdown.Reason)
		return true
	case *pb.WorkerResponse_HeartbeatAck:
		// no-op
	}
	return false
}

func (w *ValkaWorker) handleTaskSignal(signal *pb.TaskSignal) {
	if sigCh, ok := w.signalChannels.Load(signal.TaskId); ok {
		select {
		case sigCh.(chan *pb.TaskSignal) <- signal:
		default:
			log.Printf("[valka] Signal channel full for task %s, dropping signal %s",
				signal.TaskId, signal.SignalId)
		}
	}
}

func (w *ValkaWorker) handleTaskAssignment(ctx context.Context, assignment *pb.TaskAssignment) {
	// Counted from now on, so a drain waits for tasks that still wait for a slot.
	w.executing.Add(1)

	taskCtx, taskCancel := context.WithCancel(ctx)
	state := &runState{runID: assignment.TaskRunId, cancel: taskCancel}
	if prev, loaded := w.activeTasks.Swap(assignment.TaskId, state); loaded {
		// An older run of this task is still here: it is stale, stop it.
		prev.(*runState).cancel()
	}
	run := &pb.RunningTask{
		TaskId:        assignment.TaskId,
		TaskRunId:     assignment.TaskRunId,
		AttemptNumber: assignment.AttemptNumber,
	}
	w.runs.Store(assignment.TaskId, run)

	// Create signal channel for this task
	sigCh := make(chan *pb.TaskSignal, 64)
	w.signalChannels.Store(assignment.TaskId, sigCh)

	go func() {
		// Runs last: the result is in unacked before the handler stops counting.
		defer w.executing.Add(-1)
		// Take the slot here, not in the receive loop, so acks, signals and cancels keep
		// flowing while the worker is at capacity.
		w.semaphore <- struct{}{}
		defer func() {
			<-w.semaphore
			// Only this run's entries: a newer run of the same task may have replaced them.
			w.activeTasks.CompareAndDelete(assignment.TaskId, state)
			w.runs.CompareAndDelete(assignment.TaskId, run)
			w.signalChannels.CompareAndDelete(assignment.TaskId, sigCh)
		}()

		w.executeTask(taskCtx, taskCancel, assignment, sigCh)
	}()
}

func (w *ValkaWorker) handleTaskCancellation(cancellation *pb.TaskCancellation) {
	if v, ok := w.activeTasks.Load(cancellation.TaskId); ok {
		log.Printf("[valka] Task %s cancelled: %s", cancellation.TaskId, cancellation.Reason)
		v.(*runState).cancel()
	}
}

func (w *ValkaWorker) executeTask(ctx context.Context, cancel context.CancelFunc, assignment *pb.TaskAssignment, sigCh chan *pb.TaskSignal) {
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
		cancel:   cancel,
		signalCh: sigCh,
		checkpointFn: func(ctx context.Context, step, output string) error {
			return w.checkpoint(ctx, &pb.CheckpointRequest{
				TaskId:    assignment.TaskId,
				TaskRunId: assignment.TaskRunId,
				Step:      step,
				Output:    output,
			})
		},
		checkpoints: make(map[string]string, len(assignment.Checkpoints)),
	}
	for _, cp := range assignment.Checkpoints {
		tctx.checkpoints[cp.Step] = cp.Output
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

	w.deliver(&pb.TaskResult{
		TaskId:       assignment.TaskId,
		TaskRunId:    assignment.TaskRunId,
		Success:      success,
		Retryable:    retryable,
		Output:       output,
		ErrorMessage: errorMessage,
	})
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

// helloRequest opens a session. It lists the runs still executing from before this
// connection: the server reattaches each one to this connection, adopts it if its
// dispatch was lost, or cancels it if it is stale.
func (w *ValkaWorker) helloRequest() *pb.WorkerRequest {
	metaStr := ""
	if w.metadata != nil {
		data, _ := json.Marshal(w.metadata)
		metaStr = string(data)
	}
	var running []*pb.RunningTask
	w.runs.Range(func(_, v interface{}) bool {
		running = append(running, v.(*pb.RunningTask))
		return true
	})
	return &pb.WorkerRequest{
		Request: &pb.WorkerRequest_Hello{
			Hello: &pb.WorkerHello{
				WorkerId:    w.workerID,
				WorkerName:  w.name,
				Queues:      w.queues,
				Concurrency: int32(w.concurrency),
				Metadata:    metaStr,
				Running:     running,
			},
		},
	}
}

// apiKeyCreds sends the API key with every call. Valka's public port is plain gRPC unless
// a proxy terminates TLS in front of it, so the key is allowed over an insecure transport.
type apiKeyCreds string

func (k apiKeyCreds) GetRequestMetadata(_ context.Context, _ ...string) (map[string]string, error) {
	return map[string]string{"authorization": "Bearer " + string(k)}, nil
}

func (apiKeyCreds) RequireTransportSecurity() bool { return false }
