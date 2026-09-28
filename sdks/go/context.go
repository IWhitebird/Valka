package valka

import (
	"context"
	"encoding/json"
	"sync"
	"time"

	pb "github.com/valka-queue/valka/sdks/go/proto/valkav1"
)

// SignalData holds the data from a received signal.
type SignalData struct {
	SignalID string
	Name     string
	Payload  string
}

// ParsePayload parses the signal's JSON payload into the provided destination.
func (s *SignalData) ParsePayload(dest interface{}) error {
	if s.Payload == "" {
		return nil
	}
	return json.Unmarshal([]byte(s.Payload), dest)
}

// TaskContext is provided to task handlers with task metadata and logging.
// It embeds context.Context for native cancellation support.
type TaskContext struct {
	context.Context

	TaskID        string
	TaskRunID     string
	QueueName     string
	TaskName      string
	AttemptNumber int32
	RawInput      string
	RawMetadata   string

	sendFn   func(*pb.WorkerRequest)
	cancel   context.CancelFunc
	signalCh chan *pb.TaskSignal
	sigBuf   []*pb.TaskSignal

	checkpointFn  func(ctx context.Context, step, output string) error
	checkpointsMu sync.Mutex
	checkpoints   map[string]string
}

// Step runs fn as the step name, or returns its result from an earlier attempt that
// already checkpointed it. The result is checkpointed before Step returns, so a retry of
// the task skips the step. T must round-trip through encoding/json.
func Step[T any](c *TaskContext, name string, fn func() (T, error)) (T, error) {
	var value T
	found, err := c.CheckpointValue(name, &value)
	if err != nil || found {
		return value, err
	}
	value, err = fn()
	if err != nil {
		return value, err
	}
	return value, c.Checkpoint(name, value)
}

// Checkpoint durably records value as the result of step for this task.
func (c *TaskContext) Checkpoint(step string, value any) error {
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	if err := c.checkpointFn(c, step, string(data)); err != nil {
		return err
	}
	c.checkpointsMu.Lock()
	defer c.checkpointsMu.Unlock()
	c.checkpoints[step] = string(data)
	return nil
}

// CheckpointValue decodes the checkpointed result of step into dest and reports whether
// any attempt of this task recorded one.
func (c *TaskContext) CheckpointValue(step string, dest any) (bool, error) {
	c.checkpointsMu.Lock()
	output, ok := c.checkpoints[step]
	c.checkpointsMu.Unlock()
	if !ok {
		return false, nil
	}
	return true, json.Unmarshal([]byte(output), dest)
}

// Input parses the task input JSON into the provided destination.
// Returns nil if input is empty.
func (c *TaskContext) Input(dest interface{}) error {
	if c.RawInput == "" {
		return nil
	}
	return json.Unmarshal([]byte(c.RawInput), dest)
}

// Metadata parses the task metadata JSON into the provided destination.
// Returns nil if metadata is empty.
func (c *TaskContext) Metadata(dest interface{}) error {
	if c.RawMetadata == "" {
		return nil
	}
	return json.Unmarshal([]byte(c.RawMetadata), dest)
}

// Log sends an INFO-level log entry.
func (c *TaskContext) Log(message string) {
	c.logAtLevel(pb.LogLevel_LOG_LEVEL_INFO, message)
}

// Debug sends a DEBUG-level log entry.
func (c *TaskContext) Debug(message string) {
	c.logAtLevel(pb.LogLevel_LOG_LEVEL_DEBUG, message)
}

// Warn sends a WARN-level log entry.
func (c *TaskContext) Warn(message string) {
	c.logAtLevel(pb.LogLevel_LOG_LEVEL_WARN, message)
}

// Error sends an ERROR-level log entry.
func (c *TaskContext) Error(message string) {
	c.logAtLevel(pb.LogLevel_LOG_LEVEL_ERROR, message)
}

// WaitForSignal waits for a signal with the given name. Non-matching signals are buffered.
// Returns nil if the context is cancelled before a matching signal arrives.
func (c *TaskContext) WaitForSignal(name string) (*SignalData, error) {
	// Check buffer first
	for i, sig := range c.sigBuf {
		if sig.SignalName == name {
			c.sigBuf = append(c.sigBuf[:i], c.sigBuf[i+1:]...)
			c.sendSignalAck(sig.SignalId)
			return &SignalData{SignalID: sig.SignalId, Name: sig.SignalName, Payload: sig.Payload}, nil
		}
	}

	// Wait for matching signal from channel
	for {
		select {
		case <-c.Done():
			return nil, c.Err()
		case sig, ok := <-c.signalCh:
			if !ok {
				return nil, context.Canceled
			}
			if sig.SignalName == name {
				c.sendSignalAck(sig.SignalId)
				return &SignalData{SignalID: sig.SignalId, Name: sig.SignalName, Payload: sig.Payload}, nil
			}
			c.sigBuf = append(c.sigBuf, sig)
		}
	}
}

// ReceiveSignal waits for the next signal (any name). Checks buffer first.
func (c *TaskContext) ReceiveSignal() (*SignalData, error) {
	// Check buffer first
	if len(c.sigBuf) > 0 {
		sig := c.sigBuf[0]
		c.sigBuf = c.sigBuf[1:]
		c.sendSignalAck(sig.SignalId)
		return &SignalData{SignalID: sig.SignalId, Name: sig.SignalName, Payload: sig.Payload}, nil
	}

	select {
	case <-c.Done():
		return nil, c.Err()
	case sig, ok := <-c.signalCh:
		if !ok {
			return nil, context.Canceled
		}
		c.sendSignalAck(sig.SignalId)
		return &SignalData{SignalID: sig.SignalId, Name: sig.SignalName, Payload: sig.Payload}, nil
	}
}

func (c *TaskContext) sendSignalAck(signalID string) {
	c.sendFn(&pb.WorkerRequest{
		Request: &pb.WorkerRequest_SignalAck{
			SignalAck: &pb.SignalAck{
				SignalId: signalID,
			},
		},
	})
}

func (c *TaskContext) logAtLevel(level pb.LogLevel, message string) {
	entry := &pb.LogEntry{
		TaskRunId:   c.TaskRunID,
		TimestampMs: time.Now().UnixMilli(),
		Level:       level,
		Message:     message,
	}
	c.sendFn(&pb.WorkerRequest{
		Request: &pb.WorkerRequest_LogBatch{
			LogBatch: &pb.LogBatch{
				Entries: []*pb.LogEntry{entry},
			},
		},
	})
}
