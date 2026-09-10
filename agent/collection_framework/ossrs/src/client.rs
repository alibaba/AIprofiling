use crate::auth::{auth, time_rfc1123};
use crate::error::{OssError, Result};
use crate::multipart::{
    CompleteMultipartUpload, FileRange, InitiateMultipartUploadResult, Part, UploadSource,
    BLOCK_SIZE, MAX_FILE_SIZE, MIN_PART_SIZE,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use futures::future;
use reqwest::Client;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use tracing::{self as log, debug};

const MAX_RETRIES: u32 = 3;

/// OSS Client for Alibaba Cloud OSS operations
#[derive(Clone)]
pub struct OssClient {
    endpoint: String,
    bucket_name: String,
    client: Client,
    proxy: Option<String>,
}

impl OssClient {
    /// Create a new OSS client
    pub fn new(endpoint: String, bucket_name: String, proxy: Option<String>) -> Result<Self> {
        let mut client_builder = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(10);

        if let Some(ref proxy_url) = proxy {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| OssError::InvalidParameters(format!("Invalid proxy: {}", e)))?;
            client_builder = client_builder.proxy(proxy);
        }

        let client = client_builder
            .build()
            .map_err(|e| OssError::NetworkError(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            endpoint,
            bucket_name,
            client,
            proxy,
        })
    }

    /// Get the full host URL
    fn get_host(&self) -> String {
        format!("{}.{}", self.bucket_name, self.endpoint)
    }

    /// Get the full URL
    fn get_url(&self, uri: &str) -> String {
        let uri = if uri.starts_with('/') {
            uri.to_string()
        } else {
            format!("/{}", uri)
        };
        format!("https://{}{}", self.get_host(), uri)
    }

    /// Build request headers with authentication
    fn build_headers(
        &self,
        uri: &str,
        verb: &str,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
        content_length: Option<usize>,
    ) -> HashMap<String, String> {
        let host = self.get_host();
        let date = time_rfc1123();
        let content_type = "application/octet-stream";
        
        let uri = if uri.starts_with('/') {
            uri.to_string()
        } else {
            format!("/{}", uri)
        };
        let resource_uri = format!("/{}{}", self.bucket_name, uri);

        let mut oss_headers = HashMap::new();

        if let Some(acl_value) = acl {
            oss_headers.insert("x-oss-object-acl".to_string(), acl_value.to_string());
        }

        if let Some(sts_token) = sts {
            oss_headers.insert("x-oss-security-token".to_string(), sts_token.to_string());
        }

        let authorization = auth(
            ak,
            sk,
            &resource_uri,
            content_type,
            &date,
            verb,
            &oss_headers,
        );

        let mut headers = HashMap::new();
        headers.insert("Host".to_string(), host);
        headers.insert("Date".to_string(), date);
        headers.insert("Content-Type".to_string(), content_type.to_string());
        headers.insert("Authorization".to_string(), authorization);

        // Merge OSS headers
        for (k, v) in oss_headers {
            headers.insert(k, v);
        }

        if let Some(length) = content_length {
            headers.insert("Content-Length".to_string(), length.to_string());
        }

        headers
    }

    /// Initialize multipart upload
    async fn initiate_multipart_upload(
        &self,
        uri: &str,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
    ) -> Result<String> {
        let init_uri = format!("{}?uploads", uri);
        let url = self.get_url(&init_uri);
        let headers = self.build_headers(&init_uri, "POST", ak, sk, sts, acl, Some(0));

        let mut req_builder = self.client.post(&url);
        for (k, v) in headers {
            req_builder = req_builder.header(k, v);
        }

        let response = req_builder
            .send()
            .await
            .map_err(|e| OssError::NetworkError(format!("Failed to send request: {}", e)))?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            return Err(OssError::UploadFailed(status.as_u16(), body));
        }

        // Parse XML response
        let result: InitiateMultipartUploadResult = serde_xml_rs::from_str(&body)
            .map_err(|e| OssError::XmlError(format!("Failed to parse XML: {}", e)))?;

        Ok(result.upload_id)
    }

    /// Upload a single part
    async fn upload_part(
        &self,
        uri: &str,
        part_number: u32,
        upload_id: &str,
        data: Vec<u8>,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
    ) -> Result<String> {
        let part_uri = format!("{}?partNumber={}&uploadId={}", uri, part_number, upload_id);
        let url = self.get_url(&part_uri);
        let content_length = data.len();
        let headers = self.build_headers(&part_uri, "PUT", ak, sk, sts, acl, Some(content_length));

        let mut retry_count = 0;
        loop {
            let mut req_builder = self.client.put(&url);
            for (k, v) in &headers {
                req_builder = req_builder.header(k, v);
            }
            req_builder = req_builder.body(data.clone());

            match req_builder.send().await {
                Ok(response) => {
                    let status = response.status();

                    if status.is_success() {
                        let etag = response
                            .headers()
                            .get("etag")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.trim_matches('"').to_string())
                            .ok_or_else(|| {
                                OssError::UploadFailed(status.as_u16(), "Missing ETag".to_string())
                            })?;

                        return Ok(etag);
                    } else {
                        let body = response.text().await.unwrap_or_default();

                        if retry_count < MAX_RETRIES {
                            retry_count += 1;
                            log::error!(
                                "Upload part {} failed, retrying ({}/{})",
                                part_number, retry_count, MAX_RETRIES
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                            continue;
                        }

                        return Err(OssError::UploadFailed(status.as_u16(), body));
                    }
                }
                Err(e) => {
                    if retry_count < MAX_RETRIES {
                        retry_count += 1;
                        log::warn!(
                            "Network error uploading part {}, retrying ({}/{}): {}",
                            part_number, retry_count, MAX_RETRIES, e
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        continue;
                    }
                    return Err(OssError::NetworkError(format!(
                        "Failed after {} retries: {}",
                        MAX_RETRIES, e
                    )));
                }
            }
        }
    }

    /// Complete multipart upload
    async fn complete_multipart_upload(
        &self,
        uri: &str,
        upload_id: &str,
        parts: Vec<Part>,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
    ) -> Result<()> {
        let complete_uri = format!("{}?uploadId={}", uri, upload_id);
        let url = self.get_url(&complete_uri);

        let complete = CompleteMultipartUpload { parts };
        let xml_body = format!(
            r#"<CompleteMultipartUpload>{}</CompleteMultipartUpload>"#,
            complete
                .parts
                .iter()
                .map(|p| format!(
                    "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                    p.part_number, p.etag
                ))
                .collect::<Vec<_>>()
                .join("")
        );

        let content_length = xml_body.len();
        let headers = self.build_headers(
            &complete_uri,
            "POST",
            ak,
            sk,
            sts,
            acl,
            Some(content_length),
        );

        let mut req_builder = self.client.post(&url);
        for (k, v) in headers {
            req_builder = req_builder.header(k, v);
        }
        req_builder = req_builder.body(xml_body);

        let response = req_builder.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await?;
            return Err(OssError::UploadFailed(status.as_u16(), body));
        }

        Ok(())
    }

    /// Upload partial sources with gzip compression
    /// This is equivalent to the Lua async_partial_file function
    pub async fn async_partial_file(
        &self,
        oss_path: &str,
        sources: Vec<UploadSource>,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
    ) -> Result<String> {
        // Validate sources
        for source in &sources {
            match source {
                UploadSource::File(range) => {
                    let metadata = std::fs::metadata(&range.file_path)
                        .map_err(|_| OssError::FileNotFound(range.file_path.clone()))?;

                    if metadata.len() > MAX_FILE_SIZE {
                        return Err(OssError::FileTooLarge(range.file_path.clone()));
                    }
                }
                UploadSource::Bytes(_) => {}
            }
        }

        // Initialize multipart upload
        let upload_id = self
            .initiate_multipart_upload(oss_path, ak, sk, sts, acl)
            .await?;
        log::debug!("Initiated multipart upload with ID: {}", upload_id);

        let mut parts = Vec::new();
        let mut part_number = 1u32;
        let mut buffer = Vec::with_capacity(BLOCK_SIZE * 5); // Larger buffer to ensure we meet min size
        let mut compressed_buffer = Vec::new(); // Store compressed data

        // Create gzip encoder (will be recreated after each part)
        let mut encoder = Some(GzEncoder::new(Vec::new(), Compression::default()));

        // Process each source
        for source in &sources {
            match source {
                UploadSource::File(range) => {
                    let mut file = File::open(&range.file_path)
                        .map_err(|_| OssError::FileNotFound(range.file_path.clone()))?;

                    file.seek(SeekFrom::Start(range.start_offset))?;

                    let mut remaining = range.end_offset - range.start_offset + 1;

                    while remaining > 0 {
                        let to_read = std::cmp::min(BLOCK_SIZE as u64, remaining) as usize;
                        let mut chunk = vec![0u8; to_read];
                        let bytes_read = file.read(&mut chunk)?;

                        if bytes_read == 0 {
                            break;
                        }

                        // Compress the chunk
                        if let Some(ref mut enc) = encoder {
                            enc.write_all(&chunk[..bytes_read])?;
                        }
                        buffer.extend_from_slice(&chunk[..bytes_read]);

                        // Check if we should upload a part
                        if buffer.len() >= BLOCK_SIZE * 3 {
                            // Finish current encoder and get compressed data
                            let mut compressed = encoder.take().unwrap().finish()?;
                            compressed_buffer.append(&mut compressed);

                            // Only upload if we have enough compressed data OR this is getting too large
                            if compressed_buffer.len() >= MIN_PART_SIZE
                                || buffer.len() >= BLOCK_SIZE * 10
                            {
                                if !compressed_buffer.is_empty() {
                                    let etag = self
                                        .upload_part(
                                            oss_path,
                                            part_number,
                                            &upload_id,
                                            compressed_buffer.clone(),
                                            ak,
                                            sk,
                                            sts,
                                            acl,
                                        )
                                        .await?;

                                    parts.push(Part { part_number, etag });

                                    part_number += 1;
                                    compressed_buffer.clear();
                                }
                            }

                            buffer.clear();
                            // Create new encoder for next part
                            encoder = Some(GzEncoder::new(Vec::new(), Compression::default()));
                        }

                        remaining -= bytes_read as u64;
                    }
                }
                UploadSource::Bytes(data) => {
                    // Compress the bytes
                    if let Some(ref mut enc) = encoder {
                        enc.write_all(data)?;
                    }
                    buffer.extend_from_slice(data);

                    // Check if we should upload a part (same logic as above)
                    if buffer.len() >= BLOCK_SIZE * 3 {
                        let mut compressed = encoder.take().unwrap().finish()?;
                        compressed_buffer.append(&mut compressed);

                        if compressed_buffer.len() >= MIN_PART_SIZE
                            || buffer.len() >= BLOCK_SIZE * 10
                        {
                            if !compressed_buffer.is_empty() {
                                let etag = self
                                    .upload_part(
                                        oss_path,
                                        part_number,
                                        &upload_id,
                                        compressed_buffer.clone(),
                                        ak,
                                        sk,
                                        sts,
                                        acl,
                                    )
                                    .await?;

                                parts.push(Part { part_number, etag });

                                part_number += 1;
                                compressed_buffer.clear();
                            }
                        }

                        buffer.clear();
                        encoder = Some(GzEncoder::new(Vec::new(), Compression::default()));
                    }
                }
            }
        }

        // Upload remaining data
        let mut final_compressed = encoder.take().unwrap().finish()?;
        compressed_buffer.append(&mut final_compressed);

        if !compressed_buffer.is_empty() {
            // If we only have one part total, or this is truly the final part, upload it
            // OSS allows the last part to be smaller than MIN_PART_SIZE
            let etag = self
                .upload_part(
                    oss_path,
                    part_number,
                    &upload_id,
                    compressed_buffer.clone(),
                    ak,
                    sk,
                    sts,
                    acl,
                )
                .await?;

            parts.push(Part { part_number, etag });

            log::debug!(
                "Uploaded final part {} successfully ({} bytes compressed)",
                part_number,
                compressed_buffer.len()
            );
        }

        // Complete multipart upload
        self.complete_multipart_upload(oss_path, &upload_id, parts, ak, sk, sts, acl)
            .await?;

        Ok(format!("Upload completed successfully: {}", oss_path))
    }

    /// Upload multiple partial files in parallel
    pub async fn async_partial_files_iter(
        &self,
        files_list: HashMap<String, Vec<UploadSource>>,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut futures = Vec::new();

        for (oss_path, sources) in files_list {
            let ak = ak.to_string();
            let sk = sk.to_string();
            let sts = sts.map(|s| s.to_string());
            let acl = acl.map(|s| s.to_string());
            let client = self.clone();

            futures.push(tokio::spawn(async move {
                client
                    .async_partial_file(
                        &oss_path,
                        sources,
                        &ak,
                        &sk,
                        sts.as_deref(),
                        acl.as_deref(),
                    )
                    .await
            }));
        }

        let results = future::join_all(futures).await;
        let mut final_results = Vec::new();

        for res in results {
            match res {
                Ok(inner_res) => final_results.push(inner_res?),
                Err(e) => return Err(OssError::NetworkError(format!("Task join error: {}", e))),
            }
        }

        Ok(final_results)
    }

    /// Simple wrapper for single file upload
    pub async fn upload_file(
        &self,
        oss_path: &str,
        local_file: &str,
        ak: &str,
        sk: &str,
        sts: Option<&str>,
        acl: Option<&str>,
    ) -> Result<String> {
        let range = FileRange::full_file(local_file.to_string())?;
        self.async_partial_file(oss_path, vec![UploadSource::File(range)], ak, sk, sts, acl)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = OssClient::new(
            "oss-cn-hangzhou.aliyuncs.com".to_string(),
            "test-bucket".to_string(),
            None,
        );
        assert!(client.is_ok());
    }

    #[test]
    fn test_get_host() {
        let client = OssClient::new(
            "oss-cn-hangzhou.aliyuncs.com".to_string(),
            "test-bucket".to_string(),
            None,
        )
        .unwrap();

        assert_eq!(
            client.get_host(),
            "test-bucket.oss-cn-hangzhou.aliyuncs.com"
        );
    }
}
