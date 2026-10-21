package valka

import (
	"fmt"
	"os"
)

// WorkerOption configures a ValkaWorker via the functional options pattern.
type WorkerOption func(*workerConfig) error

type workerConfig struct {
	name        string
	serverAddrs []string
	apiKey      string
	queues      []string
	concurrency int
	metadata    map[string]interface{}
	handler     TaskHandler
}

func defaultConfig() *workerConfig {
	return &workerConfig{
		serverAddrs: []string{"localhost:50051"},
		apiKey:      os.Getenv("VALKA_API_KEY"),
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
		c.serverAddrs = []string{addr}
		return nil
	}
}

// WithServerAddrs sets several nodes of one cluster (host:port each). The worker connects to
// one and moves to the next when its connection drops; running tasks reattach wherever it
// lands.
func WithServerAddrs(addrs ...string) WorkerOption {
	return func(c *workerConfig) error {
		if len(addrs) == 0 {
			return fmt.Errorf("at least one server address is required")
		}
		c.serverAddrs = addrs
		return nil
	}
}

// WithAPIKey sets the API key, sent as "authorization: Bearer <key>" (default: the
// VALKA_API_KEY environment variable).
func WithAPIKey(key string) WorkerOption {
	return func(c *workerConfig) error {
		c.apiKey = key
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
