package valka

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
)

// ValkaClient is an async-friendly REST client for the Valka task queue API.
type ValkaClient struct {
	baseURL    string
	httpClient *http.Client
	headers    map[string]string
}

// NewClient creates a new Valka REST client.
func NewClient(baseURL string, opts ...ClientOption) *ValkaClient {
	c := &ValkaClient{
		baseURL:    strings.TrimRight(baseURL, "/"),
		httpClient: http.DefaultClient,
		headers:    make(map[string]string),
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

// CreateTask creates a new task.
func (c *ValkaClient) CreateTask(req CreateTaskRequest) (*Task, error) {
	var task Task
	if err := c.post("/api/v1/tasks", req, &task); err != nil {
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
	if err := c.post(fmt.Sprintf("/api/v1/tasks/%s/cancel", taskID), nil, &task); err != nil {
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

// HealthCheck checks server health.
func (c *ValkaClient) HealthCheck() (string, error) {
	req, err := http.NewRequest("GET", c.baseURL+"/healthz", nil)
	if err != nil {
		return "", err
	}
	for k, v := range c.headers {
		req.Header.Set(k, v)
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", err
	}
	return string(body), nil
}

func (c *ValkaClient) get(path string, query url.Values, dest interface{}) error {
	u := c.baseURL + path
	if len(query) > 0 {
		u += "?" + query.Encode()
	}
	req, err := http.NewRequest("GET", u, nil)
	if err != nil {
		return err
	}
	return c.doJSON(req, dest)
}

func (c *ValkaClient) post(path string, body interface{}, dest interface{}) error {
	var bodyReader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return err
		}
		bodyReader = bytes.NewReader(data)
	}
	req, err := http.NewRequest("POST", c.baseURL+path, bodyReader)
	if err != nil {
		return err
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	return c.doJSON(req, dest)
}

func (c *ValkaClient) doJSON(req *http.Request, dest interface{}) error {
	for k, v := range c.headers {
		req.Header.Set(k, v)
	}
	resp, err := c.httpClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return err
	}
	if resp.StatusCode >= 400 {
		return NewApiError(resp.StatusCode, string(data))
	}
	if dest != nil {
		return json.Unmarshal(data, dest)
	}
	return nil
}
