pub mod auth;
pub mod client;
pub mod error;
pub mod multipart;

pub use client::OssClient;
pub use error::{OssError, Result};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic() {
        // Basic test placeholder
    }
}
