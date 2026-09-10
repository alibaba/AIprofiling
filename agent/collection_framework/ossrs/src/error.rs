use thiserror::Error;

#[derive(Error, Debug)]
pub enum OssError {
    #[error("HTTP request failed: {0}")]
    RequestError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("XML parse error: {0}")]
    XmlError(String),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("File too large: {0}")]
    FileTooLarge(String),

    #[error("Upload failed: status={0}, body={1}")]
    UploadFailed(u16, String),

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("Network error: {0}")]
    NetworkError(String),
}

pub type Result<T> = std::result::Result<T, OssError>;
