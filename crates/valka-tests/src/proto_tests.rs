use valka_proto::*;

#[test]
fn test_task_assignment_construction() {
    let assignment = TaskAssignment {
        task_id: "task-123".to_string(),
        task_run_id: "run-456".to_string(),
        queue_name: "emails".to_string(),
        task_name: "send_email".to_string(),
        input: r#"{"to":"user@test.com"}"#.to_string(),
        attempt_number: 1,
        timeout_seconds: 300,
        metadata: "{}".to_string(),
    };
    assert_eq!(assignment.task_id, "task-123");
    assert_eq!(assignment.queue_name, "emails");
    assert_eq!(assignment.timeout_seconds, 300);
}

#[test]
fn test_worker_response_task_assignment_variant() {
    let response = WorkerResponse {
        response: Some(worker_response::Response::TaskAssignment(TaskAssignment {
            task_id: "t1".to_string(),
            task_run_id: "r1".to_string(),
            queue_name: "q".to_string(),
            task_name: "test".to_string(),
            input: String::new(),
            attempt_number: 1,
            timeout_seconds: 60,
            metadata: String::new(),
        })),
    };

    match response.response {
        Some(worker_response::Response::TaskAssignment(a)) => {
            assert_eq!(a.task_id, "t1");
        }
        _ => panic!("Expected TaskAssignment variant"),
    }
}

#[test]
fn test_worker_response_cancellation_variant() {
    let response = WorkerResponse {
        response: Some(worker_response::Response::TaskCancellation(
            TaskCancellation {
                task_id: "t1".to_string(),
                reason: "User cancelled".to_string(),
            },
        )),
    };

    match response.response {
        Some(worker_response::Response::TaskCancellation(c)) => {
            assert_eq!(c.task_id, "t1");
            assert_eq!(c.reason, "User cancelled");
        }
        _ => panic!("Expected TaskCancellation variant"),
    }
}

#[test]
fn test_task_event_construction() {
    let event = TaskEvent {
        event_id: "evt-1".to_string(),
        task_id: "task-1".to_string(),
        queue_name: "demo".to_string(),
        previous_status: 1,
        new_status: 3,
        worker_id: "w-1".to_string(),
        node_id: "n-1".to_string(),
        attempt_number: 2,
        error_message: String::new(),
        timestamp_ms: 1700000000000,
    };
    assert_eq!(event.event_id, "evt-1");
    assert_eq!(event.previous_status, 1);
    assert_eq!(event.new_status, 3);
    assert_eq!(event.attempt_number, 2);
    assert_eq!(event.timestamp_ms, 1700000000000);
}

#[test]
fn test_task_result_success() {
    let result = TaskResult {
        task_id: "t1".to_string(),
        task_run_id: "r1".to_string(),
        success: true,
        retryable: false,
        output: r#"{"result": 42}"#.to_string(),
        error_message: String::new(),
    };
    assert!(result.success);
    assert!(!result.retryable);
    assert!(result.error_message.is_empty());
}

#[test]
fn test_task_result_failure() {
    let result = TaskResult {
        task_id: "t1".to_string(),
        task_run_id: "r1".to_string(),
        success: false,
        retryable: true,
        output: String::new(),
        error_message: "Connection timeout".to_string(),
    };
    assert!(!result.success);
    assert!(result.retryable);
    assert_eq!(result.error_message, "Connection timeout");
}

#[test]
fn test_worker_hello_construction() {
    let hello = WorkerHello {
        worker_id: "w-123".to_string(),
        worker_name: "my-worker".to_string(),
        queues: vec!["q1".to_string(), "q2".to_string()],
        concurrency: 4,
        metadata: "{\"env\":\"prod\"}".to_string(),
    };
    assert_eq!(hello.queues.len(), 2);
    assert_eq!(hello.concurrency, 4);
}

#[test]
fn test_heartbeat_message() {
    let hb = Heartbeat {
        active_task_ids: vec!["t1".to_string(), "t2".to_string()],
        timestamp_ms: 1700000000000,
    };
    assert_eq!(hb.active_task_ids.len(), 2);
    assert_eq!(hb.timestamp_ms, 1700000000000);
}

#[test]
fn test_log_batch_and_entries() {
    let batch = LogBatch {
        entries: vec![
            LogEntry {
                task_run_id: "r1".to_string(),
                level: 1, // INFO
                message: "Processing started".to_string(),
                timestamp_ms: 1700000000000,
                metadata: String::new(),
            },
            LogEntry {
                task_run_id: "r1".to_string(),
                level: 3, // ERROR
                message: "Something failed".to_string(),
                timestamp_ms: 1700000001000,
                metadata: "{}".to_string(),
            },
        ],
    };
    assert_eq!(batch.entries.len(), 2);
    assert_eq!(batch.entries[0].level, 1);
    assert_eq!(batch.entries[1].level, 3);
}

// ─── Signal proto messages ──────────────────────────────────────────

#[test]
fn test_task_signal_construction() {
    let signal = TaskSignal {
        signal_id: "sig-1".to_string(),
        task_id: "task-1".to_string(),
        signal_name: "approve".to_string(),
        payload: r#"{"approved": true}"#.to_string(),
        timestamp_ms: 1700000000000,
    };
    assert_eq!(signal.signal_id, "sig-1");
    assert_eq!(signal.task_id, "task-1");
    assert_eq!(signal.signal_name, "approve");
    assert_eq!(signal.payload, r#"{"approved": true}"#);
    assert_eq!(signal.timestamp_ms, 1700000000000);
}

#[test]
fn test_signal_ack_construction() {
    let ack = SignalAck {
        signal_id: "sig-42".to_string(),
    };
    assert_eq!(ack.signal_id, "sig-42");
}

#[test]
fn test_worker_response_task_signal_variant() {
    let response = WorkerResponse {
        response: Some(worker_response::Response::TaskSignal(TaskSignal {
            signal_id: "s1".to_string(),
            task_id: "t1".to_string(),
            signal_name: "pause".to_string(),
            payload: String::new(),
            timestamp_ms: 0,
        })),
    };

    match response.response {
        Some(worker_response::Response::TaskSignal(s)) => {
            assert_eq!(s.signal_id, "s1");
            assert_eq!(s.signal_name, "pause");
        }
        _ => panic!("Expected TaskSignal variant"),
    }
}

#[test]
fn test_worker_request_signal_ack_variant() {
    let request = WorkerRequest {
        request: Some(worker_request::Request::SignalAck(SignalAck {
            signal_id: "sig-99".to_string(),
        })),
    };

    match request.request {
        Some(worker_request::Request::SignalAck(ack)) => {
            assert_eq!(ack.signal_id, "sig-99");
        }
        _ => panic!("Expected SignalAck variant"),
    }
}

#[test]
fn test_send_signal_request_construction() {
    let req = SendSignalRequest {
        task_id: "task-abc".to_string(),
        signal_name: "notify".to_string(),
        payload: r#"{"msg": "hello"}"#.to_string(),
    };
    assert_eq!(req.task_id, "task-abc");
    assert_eq!(req.signal_name, "notify");
    assert_eq!(req.payload, r#"{"msg": "hello"}"#);
}

#[test]
fn test_send_signal_response_construction() {
    let resp = SendSignalResponse {
        signal_id: "sig-new".to_string(),
        delivered: true,
    };
    assert_eq!(resp.signal_id, "sig-new");
    assert!(resp.delivered);

    let resp_undelivered = SendSignalResponse {
        signal_id: "sig-queued".to_string(),
        delivered: false,
    };
    assert!(!resp_undelivered.delivered);
}
