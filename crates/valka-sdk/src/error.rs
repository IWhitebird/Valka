use thiserror::Error;

#[derive(Debug, Error)]
pub enum SdkError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    #[error("Transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    #[error("Task handler error: {0}")]
    Handler(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Worker not connected")]
    NotConnected,

    #[error("Shutdown in progress")]
    ShuttingDown,
}
