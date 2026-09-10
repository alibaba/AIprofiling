use thiserror::Error;

#[derive(Error, Debug)]
pub enum SlsError {
    #[error("HTTP request failed: {0}")]
    RequestError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Encoding error: {0}")]
    EncodingError(String),

    #[error("Compression error: {0}")]
    CompressionError(String),

    #[error("PutLogs failed: status={0}, code={1:?}, body={2}")]
    PutLogsFailed(u16, Option<String>, String),

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Batch too large: {0}")]
    BatchTooLarge(String),
}

pub type Result<T> = std::result::Result<T, SlsError>;
