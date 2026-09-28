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

    #[error("Shard {0} is not owned by this node")]
    NotOwner(u16),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Storage unavailable: {0}")]
    Unavailable(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl ServerError {
    /// The operation may succeed if retried later: storage trouble or ownership in flux.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            ServerError::NotOwner(_)
                | ServerError::Unavailable(_)
                | ServerError::Storage(_)
                | ServerError::Internal(_)
        )
    }
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
            ServerError::NotOwner(_) | ServerError::Unavailable(_) => {
                tonic::Status::unavailable(err.to_string())
            }
            ServerError::InvalidArgument(_) => tonic::Status::invalid_argument(err.to_string()),
            ServerError::Storage(_) | ServerError::Internal(_) => {
                tonic::Status::internal(err.to_string())
            }
        }
    }
}
