package valka

import "encoding/json"

// TaskStatus represents a task's lifecycle status.
type TaskStatus int

const (
	TaskStatusUnspecified  TaskStatus = 0
	TaskStatusPending      TaskStatus = 1
	TaskStatusDispatching  TaskStatus = 2
	TaskStatusRunning      TaskStatus = 3
	TaskStatusCompleted    TaskStatus = 4
	TaskStatusFailed       TaskStatus = 5
	TaskStatusRetry        TaskStatus = 6
	TaskStatusDeadLetter   TaskStatus = 7
	TaskStatusCancelled    TaskStatus = 8
)

// LogLevel represents log severity.
type LogLevel int

const (
	LogLevelDebug LogLevel = 1
	LogLevelInfo  LogLevel = 2
	LogLevelWarn  LogLevel = 3
	LogLevelError LogLevel = 4
)

// Task represents a task returned by the REST API.
type Task struct {
	ID             string          `json:"id"`
	QueueName      string          `json:"queue_name"`
	TaskName       string          `json:"task_name"`
	Status         string          `json:"status"`
	Priority       int             `json:"priority"`
	MaxRetries     int             `json:"max_retries"`
	AttemptCount   int             `json:"attempt_count"`
	TimeoutSeconds int             `json:"timeout_seconds"`
	IdempotencyKey *string         `json:"idempotency_key,omitempty"`
	Input          json.RawMessage `json:"input,omitempty"`
	Metadata       json.RawMessage `json:"metadata,omitempty"`
	Output         json.RawMessage `json:"output,omitempty"`
	ErrorMessage   *string         `json:"error_message,omitempty"`
	ScheduledAt    *string         `json:"scheduled_at,omitempty"`
	CreatedAt      string          `json:"created_at"`
	UpdatedAt      string          `json:"updated_at"`
}

// TaskRun represents a task execution attempt.
type TaskRun struct {
	ID             string          `json:"id"`
	TaskID         string          `json:"task_id"`
	AttemptNumber  int             `json:"attempt_number"`
	WorkerID       *string         `json:"worker_id,omitempty"`
	AssignedNodeID *string         `json:"assigned_node_id,omitempty"`
	Status         string          `json:"status"`
	Output         json.RawMessage `json:"output,omitempty"`
	ErrorMessage   *string         `json:"error_message,omitempty"`
	LeaseExpiresAt *string         `json:"lease_expires_at,omitempty"`
	StartedAt      *string         `json:"started_at,omitempty"`
	CompletedAt    *string         `json:"completed_at,omitempty"`
	LastHeartbeat  *string         `json:"last_heartbeat,omitempty"`
}

// TaskLog represents a log entry for a task run.
type TaskLog struct {
	ID          string          `json:"id"`
	TaskRunID   string          `json:"task_run_id"`
	TimestampMs int64           `json:"timestamp_ms"`
	Level       string          `json:"level"`
	Message     string          `json:"message"`
	Metadata    json.RawMessage `json:"metadata,omitempty"`
}

// WorkerInfo represents a connected worker.
type WorkerInfo struct {
	ID            string   `json:"id"`
	Name          string   `json:"name"`
	Queues        []string `json:"queues"`
	Concurrency   int      `json:"concurrency"`
	ActiveTasks   int      `json:"active_tasks"`
	Status        string   `json:"status"`
	LastHeartbeat string   `json:"last_heartbeat"`
	ConnectedAt   string   `json:"connected_at"`
}

// DeadLetter represents a dead-lettered task.
type DeadLetter struct {
	ID           string          `json:"id"`
	TaskID       string          `json:"task_id"`
	QueueName    string          `json:"queue_name"`
	TaskName     string          `json:"task_name"`
	Input        json.RawMessage `json:"input,omitempty"`
	ErrorMessage *string         `json:"error_message,omitempty"`
	AttemptCount int             `json:"attempt_count"`
	Metadata     json.RawMessage `json:"metadata,omitempty"`
	CreatedAt    string          `json:"created_at"`
}

// TaskEvent represents a real-time task event.
type TaskEvent struct {
	EventID     string `json:"event_id"`
	TaskID      string `json:"task_id"`
	QueueName   string `json:"queue_name"`
	NewStatus   int    `json:"new_status"`
	TimestampMs int64  `json:"timestamp_ms"`
}

// CreateTaskRequest is the body for creating a task.
type CreateTaskRequest struct {
	QueueName      string      `json:"queue_name"`
	TaskName       string      `json:"task_name"`
	Input          interface{} `json:"input,omitempty"`
	Priority       *int        `json:"priority,omitempty"`
	MaxRetries     *int        `json:"max_retries,omitempty"`
	TimeoutSeconds *int        `json:"timeout_seconds,omitempty"`
	IdempotencyKey string      `json:"idempotency_key,omitempty"`
	Metadata       interface{} `json:"metadata,omitempty"`
	ScheduledAt    string      `json:"scheduled_at,omitempty"`
}

// ListTasksParams are query parameters for listing tasks.
type ListTasksParams struct {
	QueueName string
	Status    string
	Limit     int
	Offset    int
}

// ListDeadLettersParams are query parameters for listing dead letters.
type ListDeadLettersParams struct {
	QueueName string
	Limit     int
	Offset    int
}

// GetRunLogsParams are query parameters for getting run logs.
type GetRunLogsParams struct {
	Limit   int
	AfterID string
}
