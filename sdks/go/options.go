package valka

import "fmt"

// WorkerOption configures a ValkaWorker via the functional options pattern.
type WorkerOption func(*workerConfig) error

type workerConfig struct {
	name        string
	serverAddr  string
	queues      []string
	concurrency int
	metadata    map[string]interface{}
	handler     TaskHandler
}

func defaultConfig() *workerConfig {
	return &workerConfig{
		serverAddr:  "localhost:50051",
		concurrency: 1,
	}
}

func (c *workerConfig) validate() error {
	if len(c.queues) == 0 {
		return fmt.Errorf("at least one queue is required")
	}
	if c.handler == nil {
		return fmt.Errorf("a handler function is required")
	}
	if c.concurrency < 1 {
		return fmt.Errorf("concurrency must be >= 1")
	}
	return nil
}

// WithName sets the worker display name.
func WithName(name string) WorkerOption {
	return func(c *workerConfig) error {
		c.name = name
		return nil
	}
}

// WithServerAddr sets the gRPC server address (host:port).
func WithServerAddr(addr string) WorkerOption {
	return func(c *workerConfig) error {
		c.serverAddr = addr
		return nil
	}
}

// WithQueues sets the queues this worker consumes from.
func WithQueues(queues ...string) WorkerOption {
	return func(c *workerConfig) error {
		c.queues = queues
		return nil
	}
}

// WithConcurrency sets the max concurrent task handlers.
func WithConcurrency(n int) WorkerOption {
	return func(c *workerConfig) error {
		if n < 1 {
			return fmt.Errorf("concurrency must be >= 1, got %d", n)
		}
		c.concurrency = n
		return nil
	}
}

// WithMetadata sets optional worker metadata.
func WithMetadata(meta map[string]interface{}) WorkerOption {
	return func(c *workerConfig) error {
		c.metadata = meta
		return nil
	}
}

// WithHandler sets the task handler function.
func WithHandler(h TaskHandler) WorkerOption {
	return func(c *workerConfig) error {
		c.handler = h
		return nil
	}
}
