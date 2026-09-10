use crate::error::{OssError, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

pub const BLOCK_SIZE: usize = 1024 * 1024; // 1MB
pub const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024 * 1024; // 100GB
pub const MIN_PART_SIZE: usize = 100 * 1024; // 100KB - OSS minimum part size

#[derive(Debug, Serialize, Deserialize)]
pub struct InitiateMultipartUploadResult {
    #[serde(rename = "Bucket")]
    pub bucket: String,
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(rename = "UploadId")]
    pub upload_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Part {
    #[serde(rename = "PartNumber")]
    pub part_number: u32,
    #[serde(rename = "ETag")]
    pub etag: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompleteMultipartUpload {
    #[serde(rename = "Part")]
    pub parts: Vec<Part>,
}

/// Upload source specification for partial upload
#[derive(Debug, Clone)]
pub enum UploadSource {
    File(FileRange),
    Bytes(Vec<u8>),
}

/// File range specification for partial upload
#[derive(Debug, Clone)]
pub struct FileRange {
    pub file_path: String,
    pub start_offset: u64,
    pub end_offset: u64,
}

impl FileRange {
    pub fn new(file_path: String, start_offset: u64, end_offset: u64) -> Self {
        Self {
            file_path,
            start_offset,
            end_offset,
        }
    }

    pub fn full_file(file_path: String) -> Result<Self> {
        let metadata =
            std::fs::metadata(&file_path).map_err(|_| OssError::FileNotFound(file_path.clone()))?;

        Ok(Self {
            file_path,
            start_offset: 0,
            end_offset: metadata.len() - 1,
        })
    }
}

/// Read file content by chunks with gzip compression
pub struct CompressedFileReader {
    file: File,
    encoder: Option<GzEncoder<Vec<u8>>>,
    buffer: Vec<u8>,
    finished: bool,
}

impl CompressedFileReader {
    pub fn new(file_path: &Path) -> Result<Self> {
        let file = File::open(file_path)?;
        let encoder = Some(GzEncoder::new(Vec::new(), Compression::default()));

        Ok(Self {
            file,
            encoder,
            buffer: Vec::with_capacity(BLOCK_SIZE),
            finished: false,
        })
    }

    pub fn read_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }

        let mut temp_buffer = vec![0u8; BLOCK_SIZE];
        let bytes_read = self.file.read(&mut temp_buffer)?;

        if bytes_read == 0 {
            // Finish compression
            let compressed = self.encoder.take().unwrap().finish()?;
            self.finished = true;

            if compressed.is_empty() {
                return Ok(None);
            }
            return Ok(Some(compressed));
        }

        // Compress the chunk
        if let Some(ref mut enc) = self.encoder {
            enc.write_all(&temp_buffer[..bytes_read])?;
        };

        // For now, we'll accumulate data and return when we have enough
        // In production, you might want to stream this more efficiently
        Ok(Some(Vec::new())) // Will be improved for streaming
    }
}

/// Read partial file ranges with gzip compression
pub struct PartialFileReader {
    ranges: Vec<FileRange>,
    current_index: usize,
    current_file: Option<File>,
    current_offset: u64,
    encoder: Option<GzEncoder<Vec<u8>>>,
    finished: bool,
}

impl PartialFileReader {
    pub fn new(ranges: Vec<FileRange>) -> Result<Self> {
        let encoder = Some(GzEncoder::new(Vec::new(), Compression::default()));

        Ok(Self {
            ranges,
            current_index: 0,
            current_file: None,
            current_offset: 0,
            encoder,
            finished: false,
        })
    }

    pub fn read_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }

        loop {
            // Check if we need to open a new file
            if self.current_file.is_none() {
                if self.current_index >= self.ranges.len() {
                    // All files processed, finish compression
                    let compressed = self.encoder.take().unwrap().finish()?;
                    self.finished = true;

                    if compressed.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(compressed));
                }

                let range = &self.ranges[self.current_index];
                let mut file = File::open(&range.file_path)
                    .map_err(|_| OssError::FileNotFound(range.file_path.clone()))?;

                // Seek to start offset
                use std::io::Seek;
                file.seek(std::io::SeekFrom::Start(range.start_offset))?;

                self.current_file = Some(file);
                self.current_offset = range.start_offset;
            }

            let range = &self.ranges[self.current_index];
            let remaining = range.end_offset - self.current_offset + 1;

            if remaining == 0 {
                // Move to next range
                self.current_file = None;
                self.current_index += 1;
                continue;
            }

            let to_read = std::cmp::min(BLOCK_SIZE as u64, remaining) as usize;
            let mut buffer = vec![0u8; to_read];

            if let Some(ref mut file) = self.current_file {
                let bytes_read = file.read(&mut buffer)?;

                if bytes_read == 0 {
                    // Move to next range
                    self.current_file = None;
                    self.current_index += 1;
                    continue;
                }

                // Compress the chunk
                if let Some(ref mut enc) = self.encoder {
                    enc.write_all(&buffer[..bytes_read])?;
                }
                self.current_offset += bytes_read as u64;

                // For streaming, you might want to flush periodically
                // For now, we accumulate data
            }

            // Continue reading until we have enough data or finish
            if self.current_offset >= range.end_offset {
                self.current_file = None;
                self.current_index += 1;
            }

            break;
        }

        Ok(Some(Vec::new())) // Will be improved for actual streaming
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_range() {
        let range = FileRange::new("test.txt".to_string(), 0, 100);
        assert_eq!(range.start_offset, 0);
        assert_eq!(range.end_offset, 100);
    }
}
