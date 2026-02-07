use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Worker not found: {0}")]
    WorkerNotFound(String),

    #[error("Invalid task status transition: {from} -> {to}")]
    InvalidStatusTransition { from: String, to: String },

    #[error("Idempotency conflict: task already exists with key {0}")]
    IdempotencyConflict(String),

    #[error("Queue not found: {0}")]
    QueueNotFound(String),

    #[error("Task cancelled: {0}")]
    TaskCancelled(String),

    #[error("Lease expired for task: {0}")]
    LeaseExpired(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<ServerError> for tonic::Status {
    fn from(err: ServerError) -> Self {
        match &err {
            ServerError::TaskNotFound(_) | ServerError::WorkerNotFound(_) => {
                tonic::Status::not_found(err.to_string())
            }
            ServerError::InvalidStatusTransition { .. } | ServerError::TaskCancelled(_) => {
                tonic::Status::failed_precondition(err.to_string())
            }
            ServerError::IdempotencyConflict(_) => tonic::Status::already_exists(err.to_string()),
            ServerError::QueueNotFound(_) => tonic::Status::not_found(err.to_string()),
            ServerError::LeaseExpired(_) => tonic::Status::aborted(err.to_string()),
            ServerError::Database(_) => tonic::Status::internal(err.to_string()),
            ServerError::Internal(_) => tonic::Status::internal(err.to_string()),
        }
    }
}
