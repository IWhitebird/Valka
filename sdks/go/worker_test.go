package valka

import (
	"testing"
	"time"

	pb "github.com/valka-queue/valka/sdks/go/proto/valkav1"
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
