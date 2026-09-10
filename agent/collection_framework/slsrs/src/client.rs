// SLS PutLogs client.
//
// Wire protocol:
//   POST https://{project}.{endpoint}/logstores/{logstore}/shards/lb
//   Body: LZ4-block-compressed protobuf `LogGroup`
//   Headers: x-log-apiversion=0.6.0, x-log-signaturemethod=hmac-sha1,
//            x-log-bodyrawsize=<uncompressed size>,
//            x-log-compresstype=lz4, Content-MD5, Content-Type=
//            application/x-protobuf, Date=RFC1123, Host,
//            x-acs-security-token when STS credentials are in use.
//   Signature: LOG {ak}:{base64(HMAC-SHA1(sk, canonical_string))}
//
// See:
//   https://help.aliyun.com/document_detail/29026.html  (PutLogs)
//   https://help.aliyun.com/document_detail/29012.html  (auth)

use crate::auth::{authorization, content_md5, time_rfc1123};
use crate::error::{Result, SlsError};
use crate::protobuf::{encode_log_group, LogEntry, LogGroup, LogTag};
use reqwest::Client;
use std::collections::BTreeMap;
use tracing::{self as log};

const API_VERSION: &str = "0.6.0";
const SIGNATURE_METHOD: &str = "hmac-sha1";
const CONTENT_TYPE: &str = "application/x-protobuf";
const COMPRESS_TYPE: &str = "lz4";

// PutLogs limits, per Aliyun SLS docs:
//   https://help.aliyun.com/document_detail/29026.html
const MAX_ENTRIES_PER_BATCH: usize = 4096;
const MAX_UNCOMPRESSED_BYTES_PER_BATCH: usize = 3 * 1024 * 1024;

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct SlsConfig {
    /// Regional endpoint host, e.g. `cn-hangzhou.log.aliyuncs.com`.
    pub endpoint: String,
    pub project: String,
    pub logstore: String,
    pub access_key_id: String,
    pub access_key_secret: String,
    /// STS security token — required when the AK/SK pair is an STS
    /// temporary credential, must be `None` for long-lived RAM users.
    pub sts_token: Option<String>,
    /// Optional group-level topic applied to `put_logs()` batches.
    pub source: Option<String>,
    pub topic: Option<String>,
}

impl SlsConfig {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("endpoint", &self.endpoint),
            ("project", &self.project),
            ("logstore", &self.logstore),
            ("access_key_id", &self.access_key_id),
            ("access_key_secret", &self.access_key_secret),
        ] {
            if value.trim().is_empty() {
                return Err(SlsError::InvalidParameters(format!(
                    "SlsConfig.{} must not be empty",
                    name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SlsClient {
    config: SlsConfig,
    client: Client,
}

impl SlsClient {
    pub fn new(config: SlsConfig) -> Result<Self> {
        config.validate()?;
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .pool_max_idle_per_host(4)
            .build()
            .map_err(|e| SlsError::NetworkError(format!("build reqwest client: {}", e)))?;
        Ok(Self { config, client })
    }

    fn host(&self) -> String {
        format!("{}.{}", self.config.project, self.config.endpoint)
    }

    fn resource_path(&self) -> String {
        format!("/logstores/{}/shards/lb", self.config.logstore)
    }

    fn url(&self) -> String {
        format!("https://{}{}", self.host(), self.resource_path())
    }

    /// Ship a batch of raw log entries. Splits the caller's input into
    /// sub-batches that satisfy SLS's per-request limits (4096 entries and
    /// 3 MiB uncompressed body) and issues one PutLogs per sub-batch. Every
    /// sub-batch carries the config-level `topic`/`source` and no tags —
    /// use `put_log_group` for the full batch shape.
    pub async fn put_logs(&self, entries: Vec<LogEntry>) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        for chunk in self.split_entries(entries)? {
            let mut group = LogGroup::new();
            if let Some(t) = &self.config.topic {
                group = group.with_topic(t.clone());
            }
            if let Some(s) = &self.config.source {
                group = group.with_source(s.clone());
            }
            group.logs = chunk;
            self.put_log_group(group).await?;
        }
        Ok(())
    }

    /// Ship one `LogGroup` verbatim. Caller is responsible for keeping the
    /// group under the per-request limits; oversized groups return
    /// `SlsError::BatchTooLarge`.
    pub async fn put_log_group(&self, group: LogGroup) -> Result<()> {
        if group.logs.is_empty() {
            return Ok(());
        }
        if group.logs.len() > MAX_ENTRIES_PER_BATCH {
            return Err(SlsError::BatchTooLarge(format!(
                "log group has {} entries, SLS caps a single PutLogs at {}",
                group.logs.len(),
                MAX_ENTRIES_PER_BATCH
            )));
        }

        let raw = encode_log_group(&group);
        if raw.len() > MAX_UNCOMPRESSED_BYTES_PER_BATCH {
            return Err(SlsError::BatchTooLarge(format!(
                "log group encodes to {} bytes, SLS caps a single PutLogs body at {} bytes",
                raw.len(),
                MAX_UNCOMPRESSED_BYTES_PER_BATCH
            )));
        }

        // SLS uses the raw LZ4 block format (no framing, no size prefix).
        // The uncompressed size travels in the `x-log-bodyrawsize` header.
        let compressed = lz4_flex::block::compress(&raw);
        let raw_size = raw.len();
        self.send(compressed, raw_size).await
    }

    async fn send(&self, body: Vec<u8>, raw_size: usize) -> Result<()> {
        let mut retries = 0;
        loop {
            let attempt = self.send_once(&body, raw_size).await;
            match attempt {
                Ok(()) => return Ok(()),
                Err(err) if is_retryable(&err) && retries < MAX_RETRIES => {
                    retries += 1;
                    let backoff = 1u64 << (retries - 1); // 1s, 2s, 4s
                    log::warn!(
                        "SLS PutLogs attempt {} failed ({}), retrying in {}s",
                        retries,
                        err,
                        backoff
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn send_once(&self, body: &[u8], raw_size: usize) -> Result<()> {
        let date = time_rfc1123();
        let md5 = content_md5(body);

        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("x-log-apiversion".to_string(), API_VERSION.to_string());
        headers.insert(
            "x-log-signaturemethod".to_string(),
            SIGNATURE_METHOD.to_string(),
        );
        headers.insert("x-log-bodyrawsize".to_string(), raw_size.to_string());
        headers.insert("x-log-compresstype".to_string(), COMPRESS_TYPE.to_string());
        if let Some(token) = &self.config.sts_token {
            headers.insert("x-acs-security-token".to_string(), token.clone());
        }

        let auth = authorization(
            &self.config.access_key_id,
            &self.config.access_key_secret,
            "POST",
            &md5,
            CONTENT_TYPE,
            &date,
            &headers,
            &self.resource_path(),
        );

        let mut req = self
            .client
            .post(self.url())
            .header("Host", self.host())
            .header("Date", date)
            .header("Content-Type", CONTENT_TYPE)
            .header("Content-MD5", md5)
            .header("Content-Length", body.len().to_string())
            .header("Authorization", auth);
        for (k, v) in &headers {
            req = req.header(k, v);
        }

        let response = req
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| SlsError::NetworkError(format!("send PutLogs: {}", e)))?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let sls_code = response
            .headers()
            .get("x-log-requestid")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let body_text = response.text().await.unwrap_or_default();
        Err(SlsError::PutLogsFailed(status.as_u16(), sls_code, body_text))
    }

    fn split_entries(&self, entries: Vec<LogEntry>) -> Result<Vec<Vec<LogEntry>>> {
        // Approximate per-entry uncompressed size, matching the fields we
        // actually encode. We overshoot slightly to leave headroom against
        // varint length prefixes and the group-level topic/source bytes.
        fn estimate(entry: &LogEntry) -> usize {
            let mut total = 8; // Time varint + tag bytes
            for c in &entry.contents {
                total += c.key.len() + c.value.len() + 8;
            }
            total + 4
        }

        let mut chunks = Vec::new();
        let mut current = Vec::new();
        let mut current_bytes = 0usize;

        for entry in entries {
            let entry_bytes = estimate(&entry);
            if entry_bytes > MAX_UNCOMPRESSED_BYTES_PER_BATCH {
                return Err(SlsError::BatchTooLarge(format!(
                    "single LogEntry encodes to ~{} bytes, larger than SLS's {}-byte per-request cap",
                    entry_bytes, MAX_UNCOMPRESSED_BYTES_PER_BATCH
                )));
            }
            if current.len() >= MAX_ENTRIES_PER_BATCH
                || current_bytes + entry_bytes > MAX_UNCOMPRESSED_BYTES_PER_BATCH
            {
                chunks.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
            current_bytes += entry_bytes;
            current.push(entry);
        }
        if !current.is_empty() {
            chunks.push(current);
        }
        Ok(chunks)
    }
}

fn is_retryable(err: &SlsError) -> bool {
    match err {
        SlsError::NetworkError(_) | SlsError::RequestError(_) => true,
        SlsError::PutLogsFailed(status, _, _) => *status >= 500,
        _ => false,
    }
}

/// Convenience helper: build a `LogTag`.
pub fn tag(key: impl Into<String>, value: impl Into<String>) -> LogTag {
    LogTag::new(key, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protobuf::LogContent;

    fn cfg(endpoint: &str, project: &str, logstore: &str) -> SlsConfig {
        SlsConfig {
            endpoint: endpoint.to_string(),
            project: project.to_string(),
            logstore: logstore.to_string(),
            access_key_id: "ak".to_string(),
            access_key_secret: "sk".to_string(),
            sts_token: None,
            source: None,
            topic: None,
        }
    }

    #[test]
    fn new_rejects_empty_fields() {
        for c in [
            cfg("", "p", "l"),
            cfg("e", "", "l"),
            cfg("e", "p", ""),
        ] {
            assert!(SlsClient::new(c).is_err());
        }
    }

    #[test]
    fn new_accepts_valid_config() {
        let c = cfg("cn-hangzhou.log.aliyuncs.com", "proj", "store");
        assert!(SlsClient::new(c).is_ok());
    }

    #[test]
    fn resource_and_url_paths_match_spec() {
        let c = cfg("cn-hangzhou.log.aliyuncs.com", "proj", "store");
        let client = SlsClient::new(c).unwrap();
        assert_eq!(client.resource_path(), "/logstores/store/shards/lb");
        assert_eq!(
            client.url(),
            "https://proj.cn-hangzhou.log.aliyuncs.com/logstores/store/shards/lb"
        );
    }

    #[test]
    fn split_entries_respects_max_count() {
        let client = SlsClient::new(cfg("e", "p", "l")).unwrap();
        let entries = (0..MAX_ENTRIES_PER_BATCH + 5)
            .map(|i| LogEntry::new(i as u32, vec![LogContent::new("k", "v")]))
            .collect();
        let chunks = client.split_entries(entries).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), MAX_ENTRIES_PER_BATCH);
        assert_eq!(chunks[1].len(), 5);
    }

    #[test]
    fn split_entries_respects_max_bytes() {
        let client = SlsClient::new(cfg("e", "p", "l")).unwrap();
        // ~1 MiB per entry; 4 entries = ~4 MiB total, must split.
        let big_value = "x".repeat(1024 * 1024);
        let entries: Vec<_> = (0..4)
            .map(|i| {
                LogEntry::new(i as u32, vec![LogContent::new("k", big_value.clone())])
            })
            .collect();
        let chunks = client.split_entries(entries).unwrap();
        assert!(chunks.len() >= 2, "expected split, got {} chunks", chunks.len());
        for chunk in &chunks {
            let raw: usize = chunk
                .iter()
                .map(|e| {
                    e.contents.iter().map(|c| c.key.len() + c.value.len()).sum::<usize>()
                        + 16
                })
                .sum();
            assert!(
                raw < MAX_UNCOMPRESSED_BYTES_PER_BATCH,
                "chunk raw estimate {} exceeded cap",
                raw
            );
        }
    }

    #[test]
    fn split_entries_rejects_oversized_single_entry() {
        let client = SlsClient::new(cfg("e", "p", "l")).unwrap();
        let huge = "y".repeat(MAX_UNCOMPRESSED_BYTES_PER_BATCH + 1);
        let entries = vec![LogEntry::new(0, vec![LogContent::new("k", huge)])];
        assert!(matches!(
            client.split_entries(entries),
            Err(SlsError::BatchTooLarge(_))
        ));
    }

    #[test]
    fn is_retryable_matches_5xx_and_network() {
        assert!(is_retryable(&SlsError::NetworkError("boom".into())));
        assert!(is_retryable(&SlsError::PutLogsFailed(500, None, "".into())));
        assert!(is_retryable(&SlsError::PutLogsFailed(503, None, "".into())));
        assert!(!is_retryable(&SlsError::PutLogsFailed(400, None, "".into())));
        assert!(!is_retryable(&SlsError::PutLogsFailed(403, None, "".into())));
        assert!(!is_retryable(&SlsError::InvalidParameters("x".into())));
    }
}
