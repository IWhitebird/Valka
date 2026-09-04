use thiserror::Error;

#[derive(Debug, Error)]
pub enum WalError {
    #[error("object store error: {0}")]
    Store(#[from] object_store::Error),

    #[error("corrupt segment: {0}")]
    Corrupt(String),

    #[error("encoding error: {0}")]
    Encode(String),

    #[error("write-ahead log writer is closed")]
    WriterClosed,

    #[error("write not durable after retries: {0}")]
    NotDurable(String),

    #[error("shard ownership lost")]
    OwnershipLost,

    #[error("assignment conflict: {0}")]
    AssignmentConflict(String),

    #[error("invalid configuration: {0}")]
    Config(String),
}

impl From<serde_json::Error> for WalError {
    fn from(e: serde_json::Error) -> Self {
        WalError::Encode(e.to_string())
    }
}

impl From<std::io::Error> for WalError {
    fn from(e: std::io::Error) -> Self {
        WalError::Encode(e.to_string())
    }
}

impl From<WalError> for valka_core::ServerError {
    fn from(e: WalError) -> Self {
        match e {
            WalError::NotDurable(m) => valka_core::ServerError::Unavailable(m),
            WalError::OwnershipLost => {
                valka_core::ServerError::Unavailable("ownership lost".into())
            }
            WalError::WriterClosed => valka_core::ServerError::Unavailable("writer closed".into()),
            other => valka_core::ServerError::Storage(other.to_string()),
        }
    }
}
