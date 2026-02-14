package valka

import (
	"context"
	"encoding/json"
	"time"

	pb "github.com/valka-queue/valka/sdks/go/proto/valkav1"
)

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

	sendFn func(*pb.WorkerRequest)
	cancel context.CancelFunc
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
