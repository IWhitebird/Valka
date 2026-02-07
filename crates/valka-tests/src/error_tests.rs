use valka_core::ServerError;

#[test]
fn test_task_not_found_to_status() {
    let err = ServerError::TaskNotFound("task-123".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(status.message().contains("task-123"));
}

#[test]
fn test_worker_not_found_to_status() {
    let err = ServerError::WorkerNotFound("worker-abc".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(status.message().contains("worker-abc"));
}

#[test]
fn test_invalid_status_transition_to_status() {
    let err = ServerError::InvalidStatusTransition {
        from: "PENDING".to_string(),
        to: "COMPLETED".to_string(),
    };
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(status.message().contains("PENDING"));
    assert!(status.message().contains("COMPLETED"));
}

#[test]
fn test_idempotency_conflict_to_status() {
    let err = ServerError::IdempotencyConflict("key-xyz".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::AlreadyExists);
    assert!(status.message().contains("key-xyz"));
}

#[test]
fn test_task_cancelled_to_status() {
    let err = ServerError::TaskCancelled("task-456".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(status.message().contains("task-456"));
}

#[test]
fn test_error_display_messages() {
    let task_err = ServerError::TaskNotFound("t1".to_string());
    assert_eq!(format!("{task_err}"), "Task not found: t1");

    let worker_err = ServerError::WorkerNotFound("w1".to_string());
    assert_eq!(format!("{worker_err}"), "Worker not found: w1");

    let queue_err = ServerError::QueueNotFound("q1".to_string());
    assert_eq!(format!("{queue_err}"), "Queue not found: q1");

    let lease_err = ServerError::LeaseExpired("t2".to_string());
    assert_eq!(format!("{lease_err}"), "Lease expired for task: t2");

    let internal_err = ServerError::Internal("something broke".to_string());
    assert_eq!(format!("{internal_err}"), "Internal error: something broke");

    let transition_err = ServerError::InvalidStatusTransition {
        from: "A".to_string(),
        to: "B".to_string(),
    };
    assert_eq!(
        format!("{transition_err}"),
        "Invalid task status transition: A -> B"
    );
}

#[test]
fn test_queue_not_found_to_status() {
    let err = ServerError::QueueNotFound("missing-queue".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::NotFound);
}

#[test]
fn test_lease_expired_to_status() {
    let err = ServerError::LeaseExpired("task-789".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::Aborted);
}

#[test]
fn test_internal_error_to_status() {
    let err = ServerError::Internal("panic".to_string());
    let status: tonic::Status = err.into();
    assert_eq!(status.code(), tonic::Code::Internal);
}
