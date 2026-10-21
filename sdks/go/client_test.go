package valka

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync"
	"sync/atomic"
	"testing"
)

// A node that is down (503) is skipped; the create reaches the next node with the key it
// was given the first time, so a retry can never make a second task.
func TestClientRetriesCreateOnTheNextNodeWithTheSameKey(t *testing.T) {
	var keysMu sync.Mutex
	var keys []string
	down := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body map[string]interface{}
		_ = json.NewDecoder(r.Body).Decode(&body)
		keysMu.Lock()
		keys = append(keys, body["idempotency_key"].(string))
		keysMu.Unlock()
		w.WriteHeader(http.StatusServiceUnavailable)
	}))
	defer down.Close()
	var auth atomic.Value
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth.Store(r.Header.Get("Authorization"))
		var body map[string]interface{}
		_ = json.NewDecoder(r.Body).Decode(&body)
		keysMu.Lock()
		keys = append(keys, body["idempotency_key"].(string))
		keysMu.Unlock()
		w.WriteHeader(http.StatusCreated)
		_ = json.NewEncoder(w).Encode(map[string]interface{}{"id": "t1", "status": "PENDING"})
	}))
	defer up.Close()

	c := NewClient(down.URL, WithBaseURLs(up.URL), WithClientAPIKey("k1"))
	task, err := c.CreateTask(CreateTaskRequest{QueueName: "q", TaskName: "t"})
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if task.ID != "t1" {
		t.Fatalf("got %q", task.ID)
	}
	if got := auth.Load(); got != "Bearer k1" {
		t.Fatalf("authorization header %v", got)
	}
	if len(keys) != 2 || keys[0] != keys[1] || len(keys[0]) < 10 {
		t.Fatalf("both attempts must carry one generated key: %v", keys)
	}
}

// A cancel is not idempotent: it is sent once, and its error comes back as is.
func TestClientDoesNotRetryCancels(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		w.WriteHeader(http.StatusServiceUnavailable)
	}))
	defer srv.Close()
	c := NewClient(srv.URL)
	if _, err := c.CancelTask("t1"); err == nil {
		t.Fatal("expected an error")
	}
	if calls.Load() != 1 {
		t.Fatalf("cancel sent %d times", calls.Load())
	}
}

// A 4xx other than 429 is the caller's problem: no retry.
func TestClientDoesNotRetryClientErrors(t *testing.T) {
	var calls atomic.Int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		w.WriteHeader(http.StatusBadRequest)
	}))
	defer srv.Close()
	c := NewClient(srv.URL)
	if _, err := c.GetTask("t1"); err == nil {
		t.Fatal("expected an error")
	}
	if calls.Load() != 1 {
		t.Fatalf("sent %d times", calls.Load())
	}
}

func TestWorkerAPIKeyCredentials(t *testing.T) {
	md, err := apiKeyCreds("k2").GetRequestMetadata(context.Background())
	if err != nil || md["authorization"] != "Bearer k2" {
		t.Fatalf("metadata %v, %v", md, err)
	}
	if apiKeyCreds("k2").RequireTransportSecurity() {
		t.Fatal("the key must work over plain gRPC")
	}
	w, err := NewWorker(WithQueues("q"), WithHandler(func(*TaskContext) (interface{}, error) { return nil, nil }),
		WithServerAddrs("a:1", "b:2"), WithAPIKey("k2"))
	if err != nil {
		t.Fatal(err)
	}
	if len(w.serverAddrs) != 2 || w.apiKey != "k2" {
		t.Fatalf("worker config %v %q", w.serverAddrs, w.apiKey)
	}
}
