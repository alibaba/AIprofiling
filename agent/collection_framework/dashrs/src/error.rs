use thiserror::Error;

#[derive(Error, Debug)]
pub enum DashError {
    #[error("HTTP request failed: {0}")]
    RequestError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Cannot resolve client identity: {0}")]
    MissingIdentity(String),

    #[error("Tar/gzip packaging failed: {0}")]
    PackageError(String),

    #[error("Upload failed: status={0}, body={1}")]
    UploadFailed(u16, String),

    #[error("Server rejected upload: code={0}, message={1}")]
    ServerRejected(String, String),

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Response decode error: {0}")]
    DecodeError(String),
}

pub type Result<T> = std::result::Result<T, DashError>;
