package valka

import (
	"context"
	"net"
	"testing"
	"time"

	pb "github.com/IWhitebird/Valka/sdks/go/proto/valkav1"
	"google.golang.org/grpc"
	"google.golang.org/grpc/codes"
	"google.golang.org/grpc/status"
)

func newTestWorker(t *testing.T) *ValkaWorker {
	t.Helper()
	w, err := NewWorker(
		WithQueues("q"),
		WithHandler(func(*TaskContext) (interface{}, error) { return nil, nil }),
	)
	if err != nil {
		t.Fatal(err)
	}
	return w
}

func sentResults(w *ValkaWorker) []*pb.TaskResult {
	var out []*pb.TaskResult
	for {
		select {
		case req := <-w.sendCh:
			if r := req.GetTaskResult(); r != nil {
				out = append(out, r)
			}
		default:
			return out
		}
	}
}

func result(run string) *pb.TaskResult {
	return &pb.TaskResult{TaskId: "task-" + run, TaskRunId: run, Success: true}
}

func TestDeliveredResultIsKeptUntilApplied(t *testing.T) {
	w := newTestWorker(t)
	w.deliver(result("r1"))
	if got := sentResults(w); len(got) != 1 || got[0].TaskRunId != "r1" {
		t.Fatalf("sent %v", got)
	}
	if w.unackedCount() != 1 {
		t.Fatal("result must stay unacked until the server answers")
	}
	w.handleResultAck(&pb.ResultAck{TaskId: "task-r1", TaskRunId: "r1", Status: pb.ResultStatus_RESULT_STATUS_APPLIED})
	if w.unackedCount() != 0 {
		t.Fatal("APPLIED must clear the result")
	}
}

func TestStaleAckClearsTheResult(t *testing.T) {
	w := newTestWorker(t)
	w.deliver(result("r1"))
	w.handleResultAck(&pb.ResultAck{TaskRunId: "r1", Status: pb.ResultStatus_RESULT_STATUS_STALE})
	if w.unackedCount() != 0 {
		t.Fatal("STALE must clear the result")
	}
}

func TestRetryAckResendsAfterADelay(t *testing.T) {
	w := newTestWorker(t)
	w.deliver(result("r1"))
	sentResults(w)
	w.handleResultAck(&pb.ResultAck{TaskRunId: "r1", Status: pb.ResultStatus_RESULT_STATUS_RETRY})
	if w.unackedCount() != 1 {
		t.Fatal("RETRY must keep the result")
	}
	time.Sleep(resultRetryDelay + 200*time.Millisecond)
	if got := sentResults(w); len(got) != 1 || got[0].TaskRunId != "r1" {
		t.Fatalf("expected one resend, got %v", got)
	}
}

func TestUnackedResultsAreResentOnNewSession(t *testing.T) {
	w := newTestWorker(t)
	w.deliver(result("r1"))
	w.deliver(result("r2"))
	sentResults(w)
	w.resendUnacked()
	if got := sentResults(w); len(got) != 2 {
		t.Fatalf("expected both unacked results resent, got %d", len(got))
	}
}

func TestHelloListsRunsStillExecuting(t *testing.T) {
	release := make(chan struct{})
	w, err := NewWorker(
		WithQueues("q"),
		WithHandler(func(*TaskContext) (interface{}, error) {
			<-release
			return nil, nil
		}),
	)
	if err != nil {
		t.Fatal(err)
	}
	if got := w.helloRequest().GetHello().Running; len(got) != 0 {
		t.Fatalf("a fresh worker runs nothing, got %v", got)
	}
	w.handleTaskAssignment(context.Background(), &pb.TaskAssignment{
		TaskId: "t1", TaskRunId: "r1", AttemptNumber: 2, QueueName: "q", TaskName: "n",
	})
	running := w.helloRequest().GetHello().Running
	if len(running) != 1 || running[0].TaskRunId != "r1" || running[0].AttemptNumber != 2 {
		t.Fatalf("the executing run must be reported on reconnect, got %v", running)
	}
	close(release)
	deadline := time.Now().Add(5 * time.Second)
	for len(w.helloRequest().GetHello().Running) != 0 {
		if time.Now().After(deadline) {
			t.Fatal("a finished run is still reported")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func waitUntil(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting: %s", what)
		}
		time.Sleep(10 * time.Millisecond)
	}
}

// A newer run of a task replaces an older one still finishing here: the older one is
// stopped, and its cleanup leaves the newer run's state (hello, heartbeats, signals) alone.
func TestAnOlderRunFinishingKeepsTheNewerRun(t *testing.T) {
	release := make(chan struct{})
	defer close(release)
	w, err := NewWorker(
		WithQueues("q"),
		WithConcurrency(2),
		WithHandler(func(c *TaskContext) (interface{}, error) {
			if c.TaskRunID == "r1" {
				<-c.Done()
				return nil, c.Err()
			}
			<-release
			return nil, nil
		}),
	)
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	w.handleTaskAssignment(ctx, &pb.TaskAssignment{TaskId: "t1", TaskRunId: "r1"})
	w.handleTaskAssignment(ctx, &pb.TaskAssignment{TaskId: "t1", TaskRunId: "r2"})
	waitUntil(t, "the superseded run to finish", func() bool { return w.executing.Load() == 1 })
	if _, ok := w.unacked.Load("r1"); !ok {
		t.Fatal("the superseded run still reports its result")
	}
	running := w.helloRequest().GetHello().Running
	if len(running) != 1 || running[0].TaskRunId != "r2" {
		t.Fatalf("the newer run must stay listed, got %v", running)
	}
	if v, ok := w.activeTasks.Load("t1"); !ok || v.(*runState).runID != "r2" {
		t.Fatal("the newer run must stay heartbeated and cancellable")
	}
	if _, ok := w.signalChannels.Load("t1"); !ok {
		t.Fatal("the newer run must keep its signal channel")
	}
}

// Shutdown never waits on the send queue, even when nothing drains it (disconnected).
func TestShutdownNeverBlocksOnAFullSendQueue(t *testing.T) {
	w := newTestWorker(t)
	for i := 0; i < cap(w.sendCh); i++ {
		w.sendCh <- &pb.WorkerRequest{}
	}
	done := make(chan struct{})
	go func() {
		w.Shutdown()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("Shutdown blocked on a full send queue")
	}
	if !w.draining() || !w.drained() {
		t.Fatal("an idle worker drains at once")
	}
}

type nodeMsg struct {
	node string
	req  *pb.WorkerRequest
}

// fakeNode records what a worker sends and answers from a script.
type fakeNode struct {
	pb.UnimplementedWorkerServiceServer
	name         string
	log          chan nodeMsg
	onHello      []*pb.WorkerResponse
	leaveOnDrain bool // answer a GracefulShutdown with ServerShutdown and close
	reject       bool // fail every session at once
}

func (n *fakeNode) Session(stream pb.WorkerService_SessionServer) error {
	if n.reject {
		return status.Error(codes.Unauthenticated, "bad key")
	}
	for {
		req, err := stream.Recv()
		if err != nil {
			return nil
		}
		n.log <- nodeMsg{n.name, req}
		switch r := req.Request.(type) {
		case *pb.WorkerRequest_Hello:
			for _, m := range n.onHello {
				if err := stream.Send(m); err != nil {
					return err
				}
			}
		case *pb.WorkerRequest_Shutdown:
			if n.leaveOnDrain {
				return stream.Send(&pb.WorkerResponse{Response: &pb.WorkerResponse_ServerShutdown{
					ServerShutdown: &pb.ServerShutdown{Reason: "restart"},
				}})
			}
		case *pb.WorkerRequest_TaskResult:
			if err := stream.Send(&pb.WorkerResponse{Response: &pb.WorkerResponse_ResultAck{
				ResultAck: &pb.ResultAck{
					TaskId:    r.TaskResult.TaskId,
					TaskRunId: r.TaskResult.TaskRunId,
					Status:    pb.ResultStatus_RESULT_STATUS_APPLIED,
				},
			}}); err != nil {
				return err
			}
		}
	}
}

func serveNode(t *testing.T, n *fakeNode) string {
	t.Helper()
	lis, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	s := grpc.NewServer()
	pb.RegisterWorkerServiceServer(s, n)
	go func() { _ = s.Serve(lis) }()
	t.Cleanup(s.Stop)
	return lis.Addr().String()
}

// The backoff restarts only once a server answered: a node that rejects every session
// (bad API key, say) is retried ever more slowly.
func TestBackoffResetsOnlyOnceTheServerAnswers(t *testing.T) {
	rejecting := serveNode(t, &fakeNode{name: "r", log: make(chan nodeMsg, 16), reject: true})
	w, err := NewWorker(WithQueues("q"), WithServerAddr(rejecting),
		WithHandler(func(*TaskContext) (interface{}, error) { return nil, nil }))
	if err != nil {
		t.Fatal(err)
	}
	retry := DefaultRetryPolicy()
	retry.attempt = 5
	if err := w.session(context.Background(), retry); err == nil {
		t.Fatal("a rejected session must fail")
	}
	if retry.attempt != 5 {
		t.Fatalf("a rejected session must not reset the backoff, attempt=%d", retry.attempt)
	}

	bye := &pb.WorkerResponse{Response: &pb.WorkerResponse_ServerShutdown{
		ServerShutdown: &pb.ServerShutdown{Reason: "restart"},
	}}
	answering := serveNode(t, &fakeNode{name: "a", log: make(chan nodeMsg, 16), onHello: []*pb.WorkerResponse{bye}})
	w2, err := NewWorker(WithQueues("q"), WithServerAddr(answering),
		WithHandler(func(*TaskContext) (interface{}, error) { return nil, nil }))
	if err != nil {
		t.Fatal(err)
	}
	if err := w2.session(context.Background(), retry); err == nil {
		t.Fatal("a ServerShutdown ends the session")
	}
	if retry.attempt != 0 {
		t.Fatalf("an answered session resets the backoff, attempt=%d", retry.attempt)
	}
}

// A node restart in the middle of a drain: the worker moves to the next node, tells it it
// is draining, lists its running task, delivers the result there and only then returns.
func TestDrainSurvivesAServerShutdown(t *testing.T) {
	msgs := make(chan nodeMsg, 64)
	assignment := &pb.WorkerResponse{Response: &pb.WorkerResponse_TaskAssignment{
		TaskAssignment: &pb.TaskAssignment{TaskId: "t1", TaskRunId: "r1", AttemptNumber: 1},
	}}
	a := serveNode(t, &fakeNode{name: "a", log: msgs, onHello: []*pb.WorkerResponse{assignment}, leaveOnDrain: true})
	b := serveNode(t, &fakeNode{name: "b", log: msgs})

	started := make(chan struct{})
	release := make(chan struct{})
	w, err := NewWorker(
		WithServerAddrs(a, b),
		WithQueues("q"),
		WithHandler(func(*TaskContext) (interface{}, error) {
			close(started)
			<-release
			return map[string]bool{"ok": true}, nil
		}),
	)
	if err != nil {
		t.Fatal(err)
	}
	runErr := make(chan error, 1)
	go func() { runErr <- w.Run(context.Background()) }()
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("the task never started")
	}
	go w.Shutdown()

	var onB []*pb.WorkerRequest
	for len(onB) < 2 {
		select {
		case m := <-msgs:
			if m.node == "b" {
				onB = append(onB, m.req)
			}
		case <-time.After(5 * time.Second):
			t.Fatal("the worker never reached node b")
		}
	}
	hello := onB[0].GetHello()
	if hello == nil || len(hello.Running) != 1 || hello.Running[0].TaskRunId != "r1" {
		t.Fatalf("hello listing the running task first, got %v", onB[0])
	}
	if onB[1].GetShutdown() == nil {
		t.Fatalf("the drain goes on on the new node, got %v", onB[1])
	}
	select {
	case err := <-runErr:
		t.Fatalf("Run returned before the handler finished: %v", err)
	default:
	}

	close(release)
	select {
	case err := <-runErr:
		if err != nil {
			t.Fatalf("Run: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Run did not return after the drain")
	}
	var results []string
	for len(msgs) > 0 {
		m := <-msgs
		if r := m.req.GetTaskResult(); r != nil {
			results = append(results, m.node+":"+r.TaskRunId)
		}
	}
	if len(results) != 1 || results[0] != "b:r1" {
		t.Fatalf("the result goes to node b once, got %v", results)
	}
}
