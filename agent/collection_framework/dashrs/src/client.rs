// HTTP client for the AIProf dashboard collector.
//
// Wire contract (see `server/dashboard/dashboardServer.js:394`):
//   POST {endpoint}/api/results/upload
//   Content-Type: multipart/form-data
//   fields:
//     - file      : the tar.gz archive (single file, up to 3 GB)
//     - taskId    : the analysisId the server previously issued
//     - clientId  : must match a REGISTER'd client (server tolerates first-time)
//
// Server responds JSON: `{code: "Success"|..., message: string, taskId: string}`.
// Anything other than `code == "Success"` becomes `DashError::ServerRejected`;
// anything other than a 2xx HTTP status becomes `DashError::UploadFailed`.

use std::path::Path;
use std::time::Duration;

use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;
use tracing::{self as log};

use crate::error::{DashError, Result};
use crate::identity::ClientIdentity;
use crate::tar_gz::tar_gz_dir;

const MAX_RETRIES: u32 = 3;
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const UPLOAD_PATH: &str = "/api/results/upload";
const TAR_GZ_FILENAME: &str = "profiling-data.tar.gz";
const TAR_GZ_MIME: &str = "application/gzip";

#[derive(Debug, Clone)]
pub struct DashConfig {
    /// Full base URL of the collector, e.g. `http://dashboard.svc:7000`.
    /// Trailing slash is stripped when composing the upload URL.
    pub endpoint: String,
    pub identity: ClientIdentity,
    /// Per-request timeout. Defaults to 300s via `DashConfig::new_defaults`.
    pub timeout: Duration,
}

impl DashConfig {
    /// Convenience constructor with the default 300s timeout.
    pub fn new_defaults(endpoint: impl Into<String>, identity: ClientIdentity) -> Self {
        Self {
            endpoint: endpoint.into(),
            identity,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.endpoint.trim().is_empty() {
            return Err(DashError::InvalidParameters(
                "DashConfig.endpoint must not be empty".into(),
            ));
        }
        if !self.endpoint.starts_with("http://") && !self.endpoint.starts_with("https://") {
            return Err(DashError::InvalidParameters(format!(
                "DashConfig.endpoint must start with http:// or https://, got {:?}",
                self.endpoint
            )));
        }
        if self.identity.client_id.trim().is_empty() {
            return Err(DashError::InvalidParameters(
                "DashConfig.identity.client_id must not be empty".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadResponse {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, rename = "taskId")]
    pub task_id: String,
}

#[derive(Clone)]
pub struct DashClient {
    config: DashConfig,
    http: Client,
}

impl DashClient {
    pub fn new(config: DashConfig) -> Result<Self> {
        config.validate()?;
        let http = Client::builder()
            .timeout(config.timeout)
            .pool_max_idle_per_host(2)
            .build()
            .map_err(|e| DashError::NetworkError(format!("build reqwest client: {}", e)))?;
        Ok(Self { config, http })
    }

    /// Package `dir` on the fly and upload as a single tar.gz.
    pub async fn upload_dir(&self, dir: &Path, task_id: &str) -> Result<UploadResponse> {
        let bytes = tar_gz_dir(dir)?;
        log::info!(
            "dashrs: packaged {:?} into {} bytes tar.gz, uploading as task {}",
            dir,
            bytes.len(),
            task_id
        );
        self.upload_tar_gz(bytes, task_id).await
    }

    /// Upload a pre-built tar.gz body — used by `writer.rs` to avoid re-reading
    /// files off disk after materializing offsets.
    pub async fn upload_tar_gz(&self, bytes: Vec<u8>, task_id: &str) -> Result<UploadResponse> {
        if task_id.trim().is_empty() {
            return Err(DashError::InvalidParameters(
                "task_id must not be empty (dashboard requires an analysisId)".into(),
            ));
        }
        if bytes.is_empty() {
            return Err(DashError::InvalidParameters(
                "tar.gz body is empty; refusing to upload".into(),
            ));
        }

        let url = format!(
            "{}{}",
            self.config.endpoint.trim_end_matches('/'),
            UPLOAD_PATH
        );
        let client_id = self.config.identity.client_id.clone();
        let task_id = task_id.to_string();

        // Form cannot be cloned across retries — rebuild per attempt.
        let build_form = || -> Result<Form> {
            let part = Part::bytes(bytes.clone())
                .file_name(TAR_GZ_FILENAME)
                .mime_str(TAR_GZ_MIME)
                .map_err(|e| DashError::NetworkError(format!("mime_str: {}", e)))?;
            Ok(Form::new()
                .text("taskId", task_id.clone())
                .text("clientId", client_id.clone())
                .part("file", part))
        };

        let mut retries = 0;
        loop {
            let form = build_form()?;
            match self.http.post(&url).multipart(form).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let text = resp
                            .text()
                            .await
                            .map_err(|e| DashError::DecodeError(format!("read body: {}", e)))?;
                        let parsed: UploadResponse = serde_json::from_str(&text).map_err(|e| {
                            DashError::DecodeError(format!(
                                "parse response {:?}: {}",
                                truncate(&text, 256),
                                e
                            ))
                        })?;
                        if parsed.code == "Success" {
                            log::info!(
                                "dashrs: upload succeeded task={} client={}",
                                parsed.task_id,
                                client_id
                            );
                            return Ok(parsed);
                        }
                        return Err(DashError::ServerRejected(parsed.code, parsed.message));
                    }
                    let code = status.as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    if is_retryable_status(code) && retries < MAX_RETRIES {
                        retries += 1;
                        let backoff = 1u64 << (retries - 1);
                        log::warn!(
                            "dashrs: upload attempt {} got HTTP {}, retrying in {}s",
                            retries,
                            code,
                            backoff
                        );
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                        continue;
                    }
                    return Err(DashError::UploadFailed(code, truncate(&body, 512)));
                }
                Err(e) => {
                    if retries < MAX_RETRIES {
                        retries += 1;
                        let backoff = 1u64 << (retries - 1);
                        log::warn!(
                            "dashrs: upload attempt {} network error ({}), retrying in {}s",
                            retries,
                            e,
                            backoff
                        );
                        tokio::time::sleep(Duration::from_secs(backoff)).await;
                        continue;
                    }
                    return Err(DashError::NetworkError(format!("send upload: {}", e)));
                }
            }
        }
    }
}

fn is_retryable_status(code: u16) -> bool {
    code >= 500 || code == 408 || code == 429
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...(truncated {} bytes)", &s[..max], s.len() - max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ident(client_id: &str) -> ClientIdentity {
        ClientIdentity {
            client_id: client_id.to_string(),
            namespace: String::new(),
            pod_name: String::new(),
            node_name: String::new(),
            labels: HashMap::new(),
            capabilities: Vec::new(),
        }
    }

    #[test]
    fn rejects_empty_endpoint() {
        let cfg = DashConfig::new_defaults("", ident("host-1"));
        assert!(DashClient::new(cfg).is_err());
    }

    #[test]
    fn rejects_missing_scheme() {
        let cfg = DashConfig::new_defaults("dashboard.svc:7000", ident("host-1"));
        assert!(DashClient::new(cfg).is_err());
    }

    #[test]
    fn rejects_empty_client_id() {
        let cfg = DashConfig::new_defaults("http://dashboard.svc:7000", ident(""));
        assert!(DashClient::new(cfg).is_err());
    }

    #[test]
    fn accepts_valid_http_config() {
        let cfg = DashConfig::new_defaults("http://dashboard.svc:7000/", ident("host-1"));
        assert!(DashClient::new(cfg).is_ok());
    }

    #[test]
    fn accepts_valid_https_config() {
        let cfg = DashConfig::new_defaults("https://dashboard.svc/", ident("host-1"));
        assert!(DashClient::new(cfg).is_ok());
    }

    #[test]
    fn retryable_status_matches_spec() {
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(403));
        assert!(!is_retryable_status(404));
    }

    #[tokio::test]
    async fn empty_task_id_rejected() {
        let cfg = DashConfig::new_defaults("http://127.0.0.1:1", ident("host-1"));
        let client = DashClient::new(cfg).unwrap();
        let err = client.upload_tar_gz(vec![1, 2, 3], "").await.unwrap_err();
        assert!(matches!(err, DashError::InvalidParameters(_)));
    }

    #[tokio::test]
    async fn empty_body_rejected() {
        let cfg = DashConfig::new_defaults("http://127.0.0.1:1", ident("host-1"));
        let client = DashClient::new(cfg).unwrap();
        let err = client
            .upload_tar_gz(Vec::new(), "task-1")
            .await
            .unwrap_err();
        assert!(matches!(err, DashError::InvalidParameters(_)));
    }
}
