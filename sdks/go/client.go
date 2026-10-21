package valka

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync/atomic"
	"time"
)

// ValkaClient is a REST client for the Valka task queue API.
//
// Give it every node's URL (WithBaseURLs): any node takes any request, so when one goes
// away the client moves to the next. Creates carry an idempotency key (generated when the
// request has none), so a create retried after a lost response returns the task it already
// made. Reads are retried the same way; cancels and signals are not, being not idempotent.
type ValkaClient struct {
	baseURLs        []string
	next            atomic.Uint64
	httpClient      *http.Client
	headers         map[string]string
	apiKey          string
	attempts        int
	autoIdempotency bool
}

// NewClient creates a Valka REST client for baseURL (e.g. http://localhost:8989). The API
// key defaults to the VALKA_API_KEY environment variable.
func NewClient(baseURL string, opts ...ClientOption) *ValkaClient {
	c := &ValkaClient{
		baseURLs:        []string{strings.TrimRight(baseURL, "/")},
		httpClient:      &http.Client{Timeout: 10 * time.Second},
		headers:         make(map[string]string),
		apiKey:          os.Getenv("VALKA_API_KEY"),
		attempts:        6,
		autoIdempotency: true,
	}
	for _, opt := range opts {
		opt(c)
	}
	return c
}

// ClientOption configures a ValkaClient.
type ClientOption func(*ValkaClient)

// WithHTTPClient sets a custom http.Client.
func WithHTTPClient(hc *http.Client) ClientOption {
	return func(c *ValkaClient) { c.httpClient = hc }
}

// WithHeaders sets default headers for all requests.
func WithHeaders(headers map[string]string) ClientOption {
	return func(c *ValkaClient) { c.headers = headers }
}

// WithBaseURLs adds more nodes of the same cluster.
func WithBaseURLs(urls ...string) ClientOption {
	return func(c *ValkaClient) {
		for _, u := range urls {
			c.baseURLs = append(c.baseURLs, strings.TrimRight(u, "/"))
		}
	}
}

// WithClientAPIKey sets the API key, sent as "Authorization: Bearer <key>".
func WithClientAPIKey(key string) ClientOption {
	return func(c *ValkaClient) { c.apiKey = key }
}

// WithAttempts sets the attempts per call for retryable failures (1 disables retries).
func WithAttempts(n int) ClientOption {
	return func(c *ValkaClient) {
		if n < 1 {
			n = 1
		}
		c.attempts = n
	}
}

// WithAutoIdempotency turns generated idempotency keys for creates on or off (default on).
func WithAutoIdempotency(on bool) ClientOption {
	return func(c *ValkaClient) { c.autoIdempotency = on }
}

// CreateTask creates a new task. A second create with the same idempotency key returns
// the first task.
func (c *ValkaClient) CreateTask(req CreateTaskRequest) (*Task, error) {
	if req.IdempotencyKey == "" && c.autoIdempotency {
		req.IdempotencyKey = "auto-" + randomHex(16)
	}
	var task Task
	if err := c.call("POST", "/api/v1/tasks", nil, req, &task, req.IdempotencyKey != ""); err != nil {
		return nil, err
	}
	return &task, nil
}

// GetTask retrieves a task by ID.
func (c *ValkaClient) GetTask(taskID string) (*Task, error) {
	var task Task
	if err := c.get(fmt.Sprintf("/api/v1/tasks/%s", taskID), nil, &task); err != nil {
		return nil, err
	}
	return &task, nil
}

// ListTasks lists tasks with optional filters.
func (c *ValkaClient) ListTasks(params ListTasksParams) ([]Task, error) {
	query := url.Values{}
	if params.QueueName != "" {
		query.Set("queue_name", params.QueueName)
	}
	if params.Status != "" {
		query.Set("status", params.Status)
	}
	if params.Limit > 0 {
		query.Set("limit", strconv.Itoa(params.Limit))
	}
	if params.Offset > 0 {
		query.Set("offset", strconv.Itoa(params.Offset))
	}
	var tasks []Task
	if err := c.get("/api/v1/tasks", query, &tasks); err != nil {
		return nil, err
	}
	return tasks, nil
}

// CancelTask cancels a task by ID.
func (c *ValkaClient) CancelTask(taskID string) (*Task, error) {
	var task Task
	if err := c.call("POST", fmt.Sprintf("/api/v1/tasks/%s/cancel", taskID), nil, nil, &task, false); err != nil {
		return nil, err
	}
	return &task, nil
}

// GetTaskRuns retrieves execution attempts for a task.
func (c *ValkaClient) GetTaskRuns(taskID string) ([]TaskRun, error) {
	var runs []TaskRun
	if err := c.get(fmt.Sprintf("/api/v1/tasks/%s/runs", taskID), nil, &runs); err != nil {
		return nil, err
	}
	return runs, nil
}

// GetRunLogs retrieves logs for a specific task run.
func (c *ValkaClient) GetRunLogs(taskID, runID string, params GetRunLogsParams) ([]TaskLog, error) {
	query := url.Values{}
	if params.Limit > 0 {
		query.Set("limit", strconv.Itoa(params.Limit))
	}
	if params.AfterID != "" {
		query.Set("after_id", params.AfterID)
	}
	var logs []TaskLog
	if err := c.get(fmt.Sprintf("/api/v1/tasks/%s/runs/%s/logs", taskID, runID), query, &logs); err != nil {
		return nil, err
	}
	return logs, nil
}

// ListWorkers lists connected workers.
func (c *ValkaClient) ListWorkers() ([]WorkerInfo, error) {
	var workers []WorkerInfo
	if err := c.get("/api/v1/workers", nil, &workers); err != nil {
		return nil, err
	}
	return workers, nil
}

// ListDeadLetters lists dead-lettered tasks.
func (c *ValkaClient) ListDeadLetters(params ListDeadLettersParams) ([]DeadLetter, error) {
	query := url.Values{}
	if params.QueueName != "" {
		query.Set("queue_name", params.QueueName)
	}
	if params.Limit > 0 {
		query.Set("limit", strconv.Itoa(params.Limit))
	}
	if params.Offset > 0 {
		query.Set("offset", strconv.Itoa(params.Offset))
	}
	var letters []DeadLetter
	if err := c.get("/api/v1/dead-letters", query, &letters); err != nil {
		return nil, err
	}
	return letters, nil
}

// SendSignalRequest is the body for sending a signal to a task.
type SendSignalRequest struct {
	SignalName string      `json:"signal_name"`
	Payload    interface{} `json:"payload,omitempty"`
}

// SendSignalResponse is the response from sending a signal.
type SendSignalResponse struct {
	SignalID  string `json:"signal_id"`
	Delivered bool   `json:"delivered"`
}

// SendSignal sends a named signal to a running task.
func (c *ValkaClient) SendSignal(taskID, signalName string, payload interface{}) (*SendSignalResponse, error) {
	var resp SendSignalResponse
	if err := c.call("POST", fmt.Sprintf("/api/v1/tasks/%s/signal", taskID), nil, SendSignalRequest{
		SignalName: signalName,
		Payload:    payload,
	}, &resp, false); err != nil {
		return nil, err
	}
	return &resp, nil
}

// HealthCheck checks server health.
func (c *ValkaClient) HealthCheck() (string, error) {
	var body string
	err := c.retrying(true, func(base string) (bool, error) {
		req, err := http.NewRequest("GET", base+"/healthz", nil)
		if err != nil {
			return false, err
		}
		resp, err := c.httpClient.Do(req)
		if err != nil {
			return true, err
		}
		defer resp.Body.Close()
		data, err := io.ReadAll(resp.Body)
		if err != nil {
			return true, err
		}
		body = string(data)
		return false, nil
	})
	return body, err
}

func (c *ValkaClient) get(path string, query url.Values, dest interface{}) error {
	return c.call("GET", path, query, nil, dest, true)
}

// call sends one request, retrying retryable failures on the next node when retry is set.
func (c *ValkaClient) call(method, path string, query url.Values, body, dest interface{}, retry bool) error {
	var payload []byte
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return err
		}
		payload = data
	}
	return c.retrying(retry, func(base string) (bool, error) {
		u := base + path
		if len(query) > 0 {
			u += "?" + query.Encode()
		}
		var reader io.Reader
		if payload != nil {
			reader = bytes.NewReader(payload)
		}
		req, err := http.NewRequest(method, u, reader)
		if err != nil {
			return false, err
		}
		if payload != nil {
			req.Header.Set("Content-Type", "application/json")
		}
		return c.doJSON(req, dest)
	})
}

// retrying runs attempt against the current node; a retryable failure moves to the next
// node and tries again after a backoff, up to c.attempts times.
func (c *ValkaClient) retrying(retry bool, attempt func(base string) (bool, error)) error {
	attempts := 1
	if retry {
		attempts = c.attempts
	}
	delay := 100 * time.Millisecond
	var last error
	for i := 0; i < attempts; i++ {
		if i > 0 {
			time.Sleep(delay)
			if delay *= 2; delay > 3*time.Second {
				delay = 3 * time.Second
			}
		}
		n := c.next.Load()
		base := c.baseURLs[n%uint64(len(c.baseURLs))]
		retryable, err := attempt(base)
		if err == nil {
			return nil
		}
		last = err
		if !retryable {
			return err
		}
		c.next.CompareAndSwap(n, n+1)
	}
	return last
}

// doJSON sends req and decodes the answer into dest. The bool reports whether a failure
// may succeed on a retry (connection trouble, or 429/502/503/504).
func (c *ValkaClient) doJSON(req *http.Request, dest interface{}) (bool, error) {
	for k, v := range c.headers {
		req.Header.Set(k, v)
	}
	if c.apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.apiKey)
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return true, err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return true, err
	}
	if resp.StatusCode >= 400 {
		retryable := resp.StatusCode == http.StatusTooManyRequests ||
			resp.StatusCode == http.StatusBadGateway ||
			resp.StatusCode == http.StatusServiceUnavailable ||
			resp.StatusCode == http.StatusGatewayTimeout
		return retryable, NewApiError(resp.StatusCode, string(data))
	}
	if dest != nil {
		if err := json.Unmarshal(data, dest); err != nil {
			return false, err
		}
	}
	return false, nil
}

func randomHex(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		panic(errors.New("valka: no randomness for an idempotency key"))
	}
	return hex.EncodeToString(b)
}
