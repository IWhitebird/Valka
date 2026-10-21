use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("Connection error: {0}")]
    Connection(String),

    /// The server answered with an error status (its code and message).
    #[error("{}", status_text(.0))]
    Grpc(tonic::Status),

    /// The connection to the node failed (its root cause, e.g. "Connection refused").
    #[error("connection failed: {}", root_cause(.0))]
    Transport(tonic::transport::Error),

    #[error("Task handler error: {0}")]
    Handler(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Worker not connected")]
    NotConnected,

    #[error("Shutdown in progress")]
    ShuttingDown,
}

impl From<tonic::Status> for SdkError {
    fn from(status: tonic::Status) -> Self {
        SdkError::Grpc(status)
    }
}

fn status_text(status: &tonic::Status) -> String {
    format!("{:?}: {}", status.code(), status.message())
}

impl From<tonic::transport::Error> for SdkError {
    fn from(e: tonic::transport::Error) -> Self {
        SdkError::Transport(e)
    }
}

/// The innermost error of a chain: transport errors wrap the same cause several times.
fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}
