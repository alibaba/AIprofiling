use ossrs::{
    multipart::{FileRange, UploadSource},
    OssClient,
};
use dashrs::{ClientIdentity, DashClient, DashConfig};
use slsrs::{LogContent, LogEntry, SlsClient, SlsConfig};
use std::env;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::io::SeekFrom;
// Import Args struct
use crate::collector::collector_name::CollectorName;
use crate::collector::event_handler::{EventHandler, SchedulerEvent};
use crate::collector::indicator_name::IndicatorName;
use crate::command::{ProfileArgs, TransportKind};
use crate::error::ErrorCode;
use crate::meta::Meta;
use crate::plugins::plugin_adapter::CollectorState;
use crate::r#const;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use tracing as log;
use base64::{engine::general_purpose, Engine as _};

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
struct FileOffset {
    file_path: String,
    start_offset: usize,
    end_offset: usize,
    #[serde(rename = "type")]
    offset_type: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", content = "data")]
enum Offsets {
    FileOffset(FileOffset),
    Glue(String),
}

#[derive(Debug)]
pub struct Writer {
    pid_map: HashMap<i32, HashMap<CollectorName, CollectorState>>,
    offset: HashMap<String, Vec<Offsets>>,
}

impl Writer {
    pub fn new(_map: &HashMap<i32, Vec<(IndicatorName, CollectorName)>>) -> Self {
        let pid_map: HashMap<i32, HashMap<CollectorName, CollectorState>> = HashMap::new();

        Writer {
            pid_map,
            offset: HashMap::new(),
        }
    }

    pub async fn update_or_insert_collector_state(
        &mut self,
        pid: i32,
        collector: CollectorName,
        state: CollectorState,
    ) {
        self.pid_map
            .entry(pid)
            .or_insert_with(HashMap::new)
            .insert(collector, state);
    }

    /// Merge and generate the final txt only once every collector of a pid has reached
    /// >= WrittingOver. TODO: switch this to writing metadata via meta instead of
    /// reading it from the torch.json header.
    pub async fn write_json(
        &mut self,
        pid: i32,
        args: &ProfileArgs,
        _name: CollectorName,
        meta: &HashMap<i32, Meta>,
    ) {
        let mut need_write = true;

        for (_, state) in self.pid_map.get(&pid).unwrap().iter() {
            if state < &CollectorState::WrittingOver {
                need_write = false;
                break;
            }
        }

        log::debug!("need write: {} pid_map:{:#?}", need_write, self.pid_map);

        if need_write {
            if let Err(e) = self.calculate_offset_for_pid(pid, args, meta).await {
                log::error!("Failed to calculate offsets for PID {}: {}", pid, e);
                // FIXME: should we still emit an OFFSET.json on error? should we drop self.offset[pid]?
            }
        };

        log::debug!("EXIT: {:#?}", self.pid_map);
        self.is_all_pid_finish(args).await;
    }

    /// True once every collector of every pid has reached a terminal state
    /// (WrittingOver or Failed). Merge and upload must not start earlier: a pid
    /// still below that has trace data on disk that has not been folded into
    /// the offset table yet.
    fn all_collectors_finished(&self) -> bool {
        self.pid_map
            .values()
            .flat_map(|collectors| collectors.values())
            .all(|state| *state >= CollectorState::WrittingOver)
    }

    /// Demote collectors that never reported to Failed so the run can finalize.
    /// Returns the (pid, collector) pairs that were given up on.
    fn mark_pending_as_failed(&mut self) -> Vec<(i32, CollectorName)> {
        let mut abandoned = Vec::new();
        for (pid, collectors) in self.pid_map.iter_mut() {
            for (name, state) in collectors.iter_mut() {
                if *state < CollectorState::WrittingOver {
                    abandoned.push((*pid, name.clone()));
                    *state = CollectorState::Failed;
                }
            }
        }
        abandoned
    }

    async fn is_all_pid_finish(&mut self, args: &ProfileArgs) {
        let exit = self.all_collectors_finished();

        if exit {
            if self.offset.get("aggregation-kernel.json").is_some() {
                self.offset
                    .get_mut("aggregation-kernel.json")
                    .unwrap()
                    .insert(0, Offsets::Glue("[".to_string()));
                self.offset
                    .get_mut("aggregation-kernel.json")
                    .unwrap()
                    .pop(); // Drop the trailing comma
                self.offset
                    .get_mut("aggregation-kernel.json")
                    .unwrap()
                    .push(Offsets::Glue("]".to_string()));
            }

            if args.upload.unwrap_or(false) && matches!(args.transport, TransportKind::Oss) {
                log::info!("Uploading to OSS...");
                if let Err(e) = self.upload_to_oss(args).await {
                    log::error!("Failed to upload to OSS: {}", e);
                }
            } else if matches!(args.transport, TransportKind::Dashboard) {
                log::info!("Uploading to dashboard collector...");
                if let Err(e) = self.upload_to_dashboard(args).await {
                    log::error!("Failed to upload to dashboard: {}", e);
                }
            } else if matches!(args.transport, TransportKind::Sls) {
                log::info!("Uploading to SLS...");
                if let Err(e) = self.upload_to_sls(args).await {
                    log::warn!("Failed to upload to SLS (non-fatal): {}", e);
                }
            } else {
                log::info!("Generating txt file...");
                match serde_json::to_string_pretty(&self.offset) {
                    Ok(offset_result) => {
                        let output_path = if args.output.clone().unwrap() == "default" {
                            format!("{}/", r#const::DEFAULT_PATH)
                        } else {
                            args.output.clone().unwrap()
                        };

                        let offset_file_path =
                            format!("{}/{}OFFSET.json", output_path, r#const::DEFAULT_PREFIX);

                        if let Err(e) = tokio::fs::write(&offset_file_path, offset_result).await {
                            log::error!(
                                "Failed to write offset_result to {}: {}",
                                offset_file_path,
                                e
                            );
                        } else {
                            log::info!("Successfully write OFFSET.json to {}", offset_file_path);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to serialize offset data: {}", e);
                    }
                };

                if args.merge.unwrap_or(false) {
                    log::info!("Merging json files...");
                    if let Err(e) = self.merge_files_by_offset(args).await {
                        log::error!("merge_files_by_offset failed: {}", e);
                    }
                };
            }

            let _ = EventHandler::global_sender().send(SchedulerEvent::Exit);
        }
    }

    /// Give up on collectors that never reported and finalize the run with the
    /// pids that did complete. Called by the scheduler watchdog: a collector
    /// which crashed without emitting WritingFinish stays below WrittingOver,
    /// which would otherwise block is_all_pid_finish — and with it the merge
    /// and upload — for every healthy pid too.
    pub async fn finalize_pending_as_failed(&mut self, args: &ProfileArgs) {
        for (pid, name) in self.mark_pending_as_failed() {
            log::warn!(
                "Collector {:?} for pid {} never reported; marking failed so the \
                 remaining pids can still be merged",
                name,
                pid
            );
        }
        self.is_all_pid_finish(args).await;
    }

    pub async fn handle_collect_failed(
        &mut self,
        pid: i32,
        name: CollectorName,
        stop_all: bool,
        args: &ProfileArgs,
    ) {
        // Same guard as the scheduler's CollectFailed arm: a pid with a socket
        // listener but no detected collector has no entry in pid_map, and a
        // failure message from it must not panic the run.
        let Some(collectors) = self.pid_map.get_mut(&pid) else {
            log::warn!(
                "handle_collect_failed: no collector state for pid {} ({:?})",
                pid,
                name
            );
            return;
        };
        if !stop_all {
            collectors.get_mut(&name).map(|state| {
                *state = CollectorState::Failed;
            });
        } else {
            for (_, state) in collectors.iter_mut() {
                *state = CollectorState::Failed;
            }
        }

        self.is_all_pid_finish(args).await;
    }

    async fn generate_header(&self, pid: i32, meta: &HashMap<i32, Meta>) -> Result<String> {
        // Fall back to a default Meta when a collector crashed before
        // registering metadata for this pid. Emitting a minimal header lets
        // whatever trace events did land still merge into a usable file,
        // instead of discarding the whole pid (which used to hard-error and,
        // combined with the blocking event loop, wedged the run).
        let default_meta;
        let meta_entry = match meta.get(&pid) {
            Some(m) => m,
            None => {
                log::warn!(
                    "No meta for PID {}; emitting default header for partial trace",
                    pid
                );
                default_meta = Meta::new();
                &default_meta
            }
        };
        serde_json::to_string(meta_entry).map_err(|e| {
            ErrorCode::MergeFileFailed(Some(format!(
                "Failed to serialize meta data for PID {}: {}",
                pid, e
            )))
            .into_error()
        })
    }

    async fn calculate_offset_for_pid(
        &mut self,
        pid: i32,
        args: &ProfileArgs,
        meta: &HashMap<i32, Meta>,
    ) -> Result<()> {
        let header = self.generate_header(pid, meta).await?;
        let header = header[0..header.len() - 1].to_string() + ",\n\"traceEvents\": [";
        let mut pid_offset = vec![Offsets::Glue(header)];

        let path = if args.output.clone().unwrap() == "default" {
            format!("/proc/{}/root{}/", pid, r#const::DEFAULT_PATH)
        } else {
            args.output.clone().unwrap()
        };

        // Find matching files
        let mut dir = fs::read_dir(path).await?;
        let prefix = format!("{}{}", r#const::DEFAULT_PREFIX, pid);

        while let Some(entry) = dir.next_entry().await? {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            let file_path = entry.path();

            if file_name.starts_with(&prefix) {
                if file_name.ends_with("cuda-memory.pickle") {
                    let metadata = match fs::metadata(&file_path).await {
                        Ok(m) => m,
                        Err(e) => {
                            log::warn!(
                                "Failed to get metadata for {}: {}, skipping file",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    };
                    let key = format!("{}.pickle", pid); // FIXME: host
                    self.offset.insert(
                        key,
                        vec![Offsets::FileOffset(FileOffset {
                            file_path: file_path.to_string_lossy().to_string(),
                            start_offset: 0,
                            end_offset: metadata.len() as usize,
                            offset_type: "file".to_string(),
                        })],
                    );
                } else if file_name.ends_with("kernel-event.json") {
                    let metadata = match fs::metadata(&file_path).await {
                        Ok(m) => m,
                        Err(e) => {
                            log::warn!(
                                "Failed to get metadata for {}: {}, skipping file",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    };
                    let file_size = metadata.len();
                    if file_size > 2 {
                        self.offset
                            .entry("aggregation-kernel.json".to_string())
                            .or_insert(vec![])
                            .push(Offsets::FileOffset(FileOffset {
                                file_path: file_path.to_string_lossy().to_string(),
                                start_offset: 1,
                                end_offset: file_size as usize - 1,
                                offset_type: "file".to_string(),
                            }));
                        self.offset
                            .get_mut("aggregation-kernel.json")
                            .unwrap()
                            .push(Offsets::Glue(",".to_string()));
                    } else {
                        log::debug!("Delete invalid file for {}", file_path.display());
                        if let Err(e) = tokio::fs::remove_file(&file_path).await {
                            log::warn!(
                                "Failed to remove file {}: {}, skipping",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    }
                } else if file_name.ends_with("cupti.json") {
                    let start_offset = match self.find_trace_events_position(&file_path).await {
                        Ok(offset) => offset as usize + 1, // skip '['
                        Err(e) => {
                            log::warn!(
                                "Failed to find trace events position for {}: {}, skipping file",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    };
                    let end_offset_result =
                        match self.find_array_end_position(&file_path, start_offset).await {
                            Ok(offset) => offset as usize,
                            Err(e) => {
                                log::warn!(
                                    "Failed to find array end position for {}: {}, skipping file",
                                    file_path.display(),
                                    e
                                );
                                continue;
                            }
                        };

                    let end_offset = if end_offset_result > 0 {
                        end_offset_result + start_offset
                    } else {
                        start_offset
                    };

                    // Ensure end_offset >= start_offset to avoid overflow
                    if end_offset > start_offset && end_offset - start_offset > 2 {
                        pid_offset.push(Offsets::FileOffset(FileOffset {
                            file_path: file_path.to_string_lossy().to_string(),
                            start_offset,
                            end_offset,
                            offset_type: "file".to_string(),
                        }));
                        pid_offset.push(Offsets::Glue(",".to_string()));

                        self.offset
                            .entry("aggregation-kernel.json".to_string())
                            .or_insert(vec![])
                            .push(Offsets::FileOffset(FileOffset {
                                file_path: file_path.to_string_lossy().to_string(),
                                start_offset,
                                end_offset,
                                offset_type: "file".to_string(),
                            }));
                        self.offset
                            .get_mut("aggregation-kernel.json")
                            .unwrap()
                            .push(Offsets::Glue(",".to_string()));
                    } else {
                        log::debug!("Delete invalid file for {}", file_path.display());
                        if let Err(e) = tokio::fs::remove_file(&file_path).await {
                            log::warn!(
                                "Failed to remove file {}: {}, skipping",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    }
                } else if file_name.ends_with("torch-profile.json") {
                    let metadata = match fs::metadata(&file_path).await {
                        Ok(m) => m,
                        Err(e) => {
                            log::warn!(
                                "Failed to get metadata for {}: {}, skipping file",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    };
                    let file_size = metadata.len();
                    if file_size > 6 {
                        pid_offset.push(Offsets::FileOffset(FileOffset {
                            file_path: file_path.to_string_lossy().to_string(),
                            start_offset: 1,
                            end_offset: file_size as usize - 1,
                            offset_type: "file".to_string(),
                        }));
                        pid_offset.push(Offsets::Glue(",".to_string()));
                    } else {
                        log::debug!("Delete invalid file for {}", file_path.display());
                        if let Err(e) = tokio::fs::remove_file(&file_path).await {
                            log::warn!(
                                "Failed to remove file {}: {}, skipping",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    }
                } else if file_name.ends_with("memory-timeline.json") {
                    let metadata = fs::metadata(&file_path).await?;
                    let key = format!("{}_memtimeline", pid);
                    self.offset.insert(
                        key,
                        vec![Offsets::FileOffset(FileOffset {
                            file_path: file_path.to_string_lossy().to_string(),
                            start_offset: 0,
                            end_offset: metadata.len() as usize,
                            offset_type: "file".to_string(),
                        })],
                    );
                } else if file_name.ends_with(".raw") {
                    if let Err(e) = tokio::fs::remove_file(&file_path).await {
                        log::warn!(
                            "Failed to remove .raw file {}: {}, skipping",
                            file_path.display(),
                            e
                        );
                        continue;
                    }
                } else {
                    // python-function.json
                    let start_offset = match self.find_trace_events_position(&file_path).await {
                        Ok(offset) => offset as usize + 1, // skip '['
                        Err(e) => {
                            log::warn!(
                                "Failed to find trace events position for {}: {}, skipping file",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    };
                    let end_offset_result =
                        match self.find_array_end_position(&file_path, start_offset).await {
                            Ok(offset) => offset as usize,
                            Err(e) => {
                                log::warn!(
                                    "Failed to find array end position for {}: {}, skipping file",
                                    file_path.display(),
                                    e
                                );
                                continue;
                            }
                        };

                    let end_offset = if end_offset_result > 0 {
                        end_offset_result + start_offset
                    } else {
                        start_offset
                    };

                    // Ensure end_offset >= start_offset to avoid overflow
                    if end_offset > start_offset && end_offset - start_offset > 2 {
                        pid_offset.push(Offsets::FileOffset(FileOffset {
                            file_path: file_path.to_string_lossy().to_string(),
                            start_offset,
                            end_offset,
                            offset_type: "file".to_string(),
                        }));
                        pid_offset.push(Offsets::Glue(",".to_string()));
                    } else {
                        log::debug!("Delete invalid file for {}", file_path.display());
                        if let Err(e) = tokio::fs::remove_file(&file_path).await {
                            log::warn!(
                                "Failed to remove file {}: {}, skipping",
                                file_path.display(),
                                e
                            );
                            continue;
                        }
                    }
                }
            }
        }

        let key = format!("{}.json", pid);
        pid_offset.pop(); // Drop the trailing comma
        pid_offset.push(Offsets::Glue("]\n}".to_string()));

        self.offset.insert(key, pid_offset);

        Ok(())
    }

    /// Streaming search for the traceEvents field position
    async fn find_trace_events_position(&self, file_path: &Path) -> Result<usize> {
        let mut file = fs::File::open(file_path).await?;
        let mut buffer = [0; 1024]; // 1KB buffer
        let mut content = String::new();
        let mut global_position = 0;
        let mut trace_events_found = false;

        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(bytes_read) => {
                    // Convert the bytes read to a string and append to content
                    let chunk = String::from_utf8_lossy(&buffer[..bytes_read]);
                    content.push_str(&chunk);

                    // Look for "traceEvents"
                    if let Some(pos) = content.find("\"traceEvents\"") {
                        trace_events_found = true;
                        // After finding "traceEvents", search for the '[' character
                        let search_start = pos + "\"traceEvents\"".len();
                        if let Some(array_start) = content[search_start..].find('[') {
                            return Ok(global_position
                                + pos
                                + "\"traceEvents\"".len()
                                + array_start);
                        }
                    }

                    // Keep content bounded so it doesn't balloon in memory
                    if content.len() > 4096 {
                        // Make sure we don't discard the region that might contain traceEvents.
                        // Only truncate when there's still enough tail after the traceEvents marker.
                        if content.len() > 2048 {
                            // Have we seen traceEvents but not yet found the '['?
                            if trace_events_found {
                                let search_start = content.find("\"traceEvents\"").unwrap()
                                    + "\"traceEvents\"".len();
                                // If we still haven't found '[', keep the relevant tail
                                if content[search_start..].find('[').is_none() {
                                    // Keep everything from traceEvents onward
                                    let pos = content.find("\"traceEvents\"").unwrap();
                                    // Fix: use char index, not byte index
                                    let pos_chars = content
                                        .char_indices()
                                        .position(|(idx, _)| idx == pos)
                                        .unwrap_or(0);
                                    let retain_content =
                                        content.chars().skip(pos_chars).collect::<String>();
                                    global_position += pos;
                                    content = retain_content;
                                } else {
                                    // '[' already found; safe to truncate
                                    let truncate_from = content.len() - 2048;
                                    // Fix: use char index, not byte index
                                    let truncate_from_chars = content
                                        .char_indices()
                                        .position(|(idx, _)| idx == truncate_from)
                                        .unwrap_or(content.len());
                                    content = content
                                        .chars()
                                        .skip(truncate_from_chars)
                                        .collect::<String>();
                                    global_position += truncate_from;
                                }
                            } else {
                                // traceEvents not seen yet; safe to truncate
                                let truncate_from = content.len() - 2048;
                                // Fix: use char index, not byte index
                                let truncate_from_chars = content
                                    .char_indices()
                                    .position(|(idx, _)| idx == truncate_from)
                                    .unwrap_or(content.len());
                                content = content
                                    .chars()
                                    .skip(truncate_from_chars)
                                    .collect::<String>();
                                global_position += truncate_from;
                            }
                        }
                    }
                }
                Err(e) => return Err(ErrorCode::MergeFileFailed(Some(e.to_string())).into_error()),
            }
        }

        Err(ErrorCode::MergeFileFailed(Some(format!(
            "traceEvents not found: {}",
            file_path.to_str().unwrap_or("")
        )))
        .into_error())
    }

    /// Streaming search for the array end position
    async fn find_array_end_position(
        &self,
        file_path: &Path,
        start_position: usize,
    ) -> Result<usize, std::io::Error> {
        let mut file = fs::File::open(file_path).await?;
        // Seek to the start position
        file.seek(SeekFrom::Start(start_position as u64)).await?;

        let mut buffer = [0; 1024]; // 1KB buffer
        let mut position = 0;
        let mut bracket_count = 1; // We start at '[', so count is 1
        let mut in_string = false;
        let mut escape_next = false;

        loop {
            match file.read(&mut buffer).await {
                Ok(0) => break, // EOF
                Ok(bytes_read) => {
                    for i in 0..bytes_read {
                        let byte = buffer[i];
                        // Convert u8 to char properly
                        let ch = byte as char;

                        if escape_next {
                            escape_next = false;
                            position += 1;
                            continue;
                        }

                        if ch == '\\' && in_string {
                            escape_next = true;
                            position += 1;
                            continue;
                        }

                        if ch == '"' {
                            in_string = !in_string;
                        }

                        if !in_string {
                            match ch {
                                '[' => bracket_count += 1,
                                ']' => {
                                    bracket_count -= 1;
                                    if bracket_count == 0 {
                                        return Ok(position); // Found matching ']'
                                    }
                                }
                                _ => {}
                            }
                        }
                        position += 1;
                    }
                }
                Err(e) => return Err(e),
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Array end not found",
        ))
    }

    pub async fn merge_files_by_offset(&self, args: &ProfileArgs) -> Result<()> {
        // Collect every source shard path and delete them only after all output
        // files are merged. Otherwise, a source referenced by multiple outputs
        // (e.g. cupti.json feeds both aggregation-kernel.json and <pid>.json)
        // gets removed on the first pass and the second merge fails with ENOENT.
        let mut sources_to_remove: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for (output_name, offsets) in &self.offset {
            let output_path = if args.output.clone().unwrap() == "default" {
                format!(
                    "{}/{}{}",
                    r#const::DEFAULT_PATH,
                    r#const::DEFAULT_PREFIX,
                    output_name
                )
            } else {
                format!(
                    "{}/{}{}",
                    args.output.clone().unwrap(),
                    r#const::DEFAULT_PREFIX,
                    output_name
                )
            };
            log::debug!("Merging to output_path: {}", output_path);
            let mut output_file = fs::File::create(&output_path).await?;

            for offset in offsets {
                match offset {
                    Offsets::FileOffset(file_offset) => {
                        if file_offset.end_offset - file_offset.start_offset > 1 {
                            let mut file = fs::File::open(&file_offset.file_path).await?;
                            file.seek(SeekFrom::Start(file_offset.start_offset as u64))
                                .await?;

                            let mut remaining_bytes =
                                file_offset.end_offset - file_offset.start_offset;
                            let mut buffer = [0; 8192];

                            while remaining_bytes > 0 {
                                let bytes_to_read = std::cmp::min(buffer.len(), remaining_bytes);
                                let bytes_read = file.read(&mut buffer[..bytes_to_read]).await?;

                                if bytes_read == 0 {
                                    break;
                                }

                                output_file.write_all(&buffer[..bytes_read]).await?;
                                remaining_bytes -= bytes_read;
                            }
                            sources_to_remove.insert(file_offset.file_path.clone());
                        }
                    }
                    Offsets::Glue(glue_data) => {
                        // Write the glue bytes directly
                        output_file.write_all(glue_data.as_bytes()).await?;
                    }
                }
            }

            output_file.flush().await?;
        }
        for src in sources_to_remove {
            if let Err(e) = fs::remove_file(&src).await {
                log::warn!("Failed to remove merged source {}: {}", src, e);
            }
        }
        Ok(())
    }

    pub async fn upload_to_oss(&self, args: &ProfileArgs) -> Result<()> {
        let ak_encoded = args
            .ak
            .clone()
            .or_else(|| env::var("OSS_AK").ok())
            .ok_or_else(|| anyhow::anyhow!("OSS_AK not set"))?;
        let sk_encoded: String = args
            .sk
            .clone()
            .or_else(|| env::var("OSS_SK").ok())
            .ok_or_else(|| anyhow::anyhow!("OSS_SK not set"))?;
        let sts_encoded = args
            .sts
            .clone()
            .or_else(|| env::var("OSS_STS").ok())
            .ok_or_else(|| anyhow::anyhow!("STS_TOKEN not set"))?;
        let analysis_id = args
            .analysis_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Analysis Id not set"))?;
        let ak = String::from_utf8(general_purpose::STANDARD.decode(ak_encoded)?)?;
        let sk = String::from_utf8(general_purpose::STANDARD.decode(sk_encoded)?)?;
        let sts = String::from_utf8(general_purpose::STANDARD.decode(sts_encoded)?)?;

        let endpoint =
            env::var("OSS_ENDPOINT").map_err(|_| anyhow::anyhow!("OSS_ENDPOINT not set"))?;
        let bucket = env::var("OSS_BUCKET").map_err(|_| anyhow::anyhow!("OSS_BUCKET not set"))?;

        let client = OssClient::new(endpoint, bucket, None)?;
        let mut files_list = HashMap::new();

        for (output_name, offsets) in &self.offset {
            let mut sources = Vec::new();
            for offset in offsets {
                match offset {
                    Offsets::FileOffset(fo) => {
                        if fo.end_offset > fo.start_offset {
                            sources.push(UploadSource::File(FileRange::new(
                                fo.file_path.clone(),
                                fo.start_offset as u64,
                                fo.end_offset as u64 - 1,
                            )));
                        }
                    }
                    Offsets::Glue(data) => {
                        sources.push(UploadSource::Bytes(data.as_bytes().to_vec()));
                    }
                }
            }
            if !sources.is_empty() {
                let mut prefix = "gpu_profiling";
                if output_name.contains("pickle") || output_name.contains("memtimeline") {
                    prefix = "snapshot"
                }

                let oss_path_gz = format!("{}/{}_{}.gz", prefix, analysis_id, output_name);
                files_list.insert(oss_path_gz, sources);
            }
        }

        if !files_list.is_empty() {
            match client
                .async_partial_files_iter(files_list, &ak, &sk, Some(&sts), Some("private"))
                .await
            {
                Ok(_) => {
                    log::info!("All files uploaded successfully");
                    // Cleanup local files
                    for (_, offsets) in &self.offset {
                        for offset in offsets {
                            if let Offsets::FileOffset(fo) = offset {
                                let _ = fs::remove_file(&fo.file_path).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    log::error!("Batch upload failed: {}", e);
                    return Err(anyhow::anyhow!("Batch upload failed: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Dashboard path: materialize the offset table to disk (via
    /// merge_files_by_offset), then tar.gz the whole output directory and POST
    /// it as multipart to the dashboard collector. task_id reuses `--analysis-id`
    /// (validate() already ensures it's non-empty).
    pub async fn upload_to_dashboard(&self, args: &ProfileArgs) -> Result<()> {
        let endpoint = args
            .dashboard_endpoint
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--dashboard-endpoint not set"))?;
        let task_id = args
            .analysis_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--analysis-id not set (required as taskId)"))?;

        let output_dir = if args.output.clone().unwrap() == "default" {
            r#const::DEFAULT_PATH.to_string()
        } else {
            args.output.clone().unwrap()
        };

        // 1) Materialize the offset table into the output directory so all
        //    files exist on disk. merge_files_by_offset removes source shards,
        //    which avoids duplicates in the tar.
        if let Err(e) = self.merge_files_by_offset(args).await {
            log::warn!(
                "merge_files_by_offset failed before dashboard tar: {} (continuing)",
                e
            );
        }

        // 2) Package into tar.gz.
        let dir_path = Path::new(&output_dir);
        let bytes = dashrs::tar_gz_dir(dir_path)
            .map_err(|e| anyhow::anyhow!("tar.gz output dir {}: {}", output_dir, e))?;
        log::info!(
            "dashboard: packaged {} into {} bytes tar.gz",
            output_dir,
            bytes.len()
        );

        // 3) Resolve the client identity, build the client, POST.
        let identity = ClientIdentity::resolve(
            args.client_id.as_deref(),
            args.namespace.as_deref(),
            args.pod_name.as_deref(),
            args.node_name.as_deref(),
        )
        .map_err(|e| anyhow::anyhow!("resolve client identity: {}", e))?;
        log::info!(
            "dashboard: client_id={} namespace={} pod={} node={}",
            identity.client_id,
            identity.namespace,
            identity.pod_name,
            identity.node_name
        );

        let cfg = DashConfig::new_defaults(endpoint, identity);
        let client = DashClient::new(cfg)
            .map_err(|e| anyhow::anyhow!("build DashClient: {}", e))?;
        match client.upload_tar_gz(bytes, &task_id).await {
            Ok(resp) => {
                log::info!(
                    "dashboard upload OK: taskId={} code={} message={}",
                    resp.task_id,
                    resp.code,
                    resp.message
                );
                Ok(())
            }
            Err(e) => {
                log::error!("dashboard upload failed: {}", e);
                Err(anyhow::anyhow!("dashboard upload failed: {}", e))
            }
        }
    }

    /// SLS path: materialize the offset table to disk (via merge_files_by_offset),
    /// then flatten every chrome-trace JSON under the output dir (cupti.json /
    /// kernel-event.json / torch-profile.json / ...) into a stream of LogEntry
    /// and PutLogs into the given logstore. analysis_id becomes the topic,
    /// client_id the source.
    ///
    /// Doesn't touch the hot collection loop — reads only files already on
    /// disk; failures don't block the main pipeline (the caller already
    /// log::warn's them, so Err here just drives the warning log).
    pub async fn upload_to_sls(&self, args: &ProfileArgs) -> Result<()> {
        let analysis_id = args
            .analysis_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--analysis-id not set (used as SLS topic)"))?;

        let output_dir = if args.output.clone().unwrap() == "default" {
            r#const::DEFAULT_PATH.to_string()
        } else {
            args.output.clone().unwrap()
        };

        if let Err(e) = self.merge_files_by_offset(args).await {
            log::warn!(
                "merge_files_by_offset failed before SLS upload: {} (continuing)",
                e
            );
        }

        let identity = ClientIdentity::resolve(
            args.client_id.as_deref(),
            args.namespace.as_deref(),
            args.pod_name.as_deref(),
            args.node_name.as_deref(),
        )
        .map_err(|e| anyhow::anyhow!("resolve client identity: {}", e))?;

        let entries =
            sls_events_from_dir(Path::new(&output_dir), &analysis_id, &identity.client_id)
                .map_err(|e| anyhow::anyhow!("scan output dir for chrome-trace JSON: {}", e))?;
        if entries.is_empty() {
            log::warn!("sls: no chrome-trace events found under {}", output_dir);
            return Ok(());
        }

        let dry_run = env::var("AIPROF_SLS_DRY_RUN")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if dry_run {
            log::info!(
                "sls: batched {} LogEntry, would put_logs (dry run)",
                entries.len()
            );
            return Ok(());
        }

        let cfg = SlsConfig {
            endpoint: args
                .sls_endpoint
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--sls-endpoint not set"))?,
            project: args
                .sls_project
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--sls-project not set"))?,
            logstore: args
                .sls_logstore
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--sls-logstore not set"))?,
            access_key_id: args.sls_ak_id.clone().unwrap_or_default(),
            access_key_secret: args.sls_ak_secret.clone().unwrap_or_default(),
            sts_token: args.sls_sts_token.clone().filter(|s| !s.trim().is_empty()),
            source: Some(identity.client_id.clone()),
            topic: Some(analysis_id.clone()),
        };
        let client =
            SlsClient::new(cfg).map_err(|e| anyhow::anyhow!("build SlsClient: {}", e))?;
        let n = entries.len();
        client
            .put_logs(entries)
            .await
            .map_err(|e| anyhow::anyhow!("SLS put_logs: {}", e))?;
        log::info!(
            "sls: PutLogs OK, {} entries (project={} logstore={} topic={})",
            n,
            args.sls_project.as_deref().unwrap_or(""),
            args.sls_logstore.as_deref().unwrap_or(""),
            analysis_id,
        );
        Ok(())
    }
}

/// Scan `dir` for chrome-trace family JSON files and turn every trace event
/// into a `LogEntry`. Base fields: analysis_id / client_id / source_file /
/// pid / tid / cat / name / ph / ts_us / dur_us / args_json. Returns an empty
/// Vec for an empty directory.
///
/// Extracted as a pure function from the inline logic in `upload_to_sls` to
/// make it easy to unit-test — no disk writes or uploads happen here.
pub fn sls_events_from_dir(
    dir: &Path,
    analysis_id: &str,
    client_id: &str,
) -> Result<Vec<LogEntry>> {
    use std::fs;

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out: Vec<LogEntry> = Vec::new();
    let entries = fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("read_dir {}: {}", dir.display(), e))?;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                log::warn!("sls: skip unreadable dir entry: {}", e);
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if !is_chrome_trace_file(&file_name) {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("sls: skip unreadable {}: {}", path.display(), e);
                continue;
            }
        };
        let parsed: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("sls: skip malformed JSON {}: {}", path.display(), e);
                continue;
            }
        };
        // chrome-trace allows two shapes: {"traceEvents":[...]} or a bare array [...]
        let events = parsed
            .get("traceEvents")
            .and_then(|v| v.as_array())
            .cloned()
            .or_else(|| parsed.as_array().cloned())
            .unwrap_or_default();
        for ev in events {
            let ts_us = ev
                .get("ts")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            // ts is microseconds (chrome-trace convention); convert to seconds for SLS Time.
            let ts_secs = (ts_us / 1_000_000.0) as u32;
            let dur_us = ev.get("dur").and_then(|v| v.as_f64());
            let pid = ev.get("pid").map(|v| v.to_string()).unwrap_or_default();
            let tid = ev.get("tid").map(|v| v.to_string()).unwrap_or_default();
            let ph = ev
                .get("ph")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cat = ev
                .get("cat")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = ev
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args_json = ev
                .get("args")
                .map(|v| v.to_string())
                .unwrap_or_default();

            let mut contents = vec![
                LogContent::new("analysis_id", analysis_id),
                LogContent::new("client_id", client_id),
                LogContent::new("source_file", &file_name),
                LogContent::new("pid", pid),
                LogContent::new("tid", tid),
                LogContent::new("cat", cat),
                LogContent::new("name", name),
                LogContent::new("ph", ph),
                LogContent::new("ts_us", format!("{}", ts_us as i64)),
            ];
            if let Some(d) = dur_us {
                contents.push(LogContent::new("dur_us", format!("{}", d as i64)));
            }
            if !args_json.is_empty() && args_json != "null" {
                contents.push(LogContent::new("args_json", args_json));
            }
            out.push(LogEntry::new(ts_secs, contents));
        }
    }
    Ok(out)
}

fn is_chrome_trace_file(name: &str) -> bool {
    // Aligned with the chrome-trace family recognized by calculate_offset_for_pid.
    name.ends_with("cupti.json")
        || name.ends_with("kernel-event.json")
        || name.ends_with("torch-profile.json")
        || name.ends_with("aggregation-kernel.json")
        || name.ends_with("memory-timeline.json")
}

#[cfg(test)]
mod sls_tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn write_file(dir: &Path, name: &str, body: &str) {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn empty_dir_returns_empty() {
        let d = tempdir().unwrap();
        let v = sls_events_from_dir(d.path(), "a1", "c1").unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn nonexistent_dir_returns_empty() {
        let v = sls_events_from_dir(Path::new("/does/not/exist"), "a1", "c1").unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn parses_trace_events_object_shape() {
        let d = tempdir().unwrap();
        let body = r#"{
            "baseTimeNanoseconds": 0,
            "traceEvents": [
                {"ph":"X","cat":"cuda_kernel","name":"gemm","pid":42,"tid":7,
                 "ts":1500000.0,"dur":500.0,"args":{"grid":[1,1,1]}},
                {"ph":"B","cat":"nccl","name":"AllReduce","pid":42,"tid":8,
                 "ts":2000000.0}
            ]
        }"#;
        write_file(d.path(), "cupti.json", body);
        let v = sls_events_from_dir(d.path(), "task-9", "host-1").unwrap();
        assert_eq!(v.len(), 2);

        let first = &v[0];
        assert_eq!(first.time_unix_secs, 1); // 1_500_000us / 1e6 = 1s
        let kv: std::collections::HashMap<_, _> = first
            .contents
            .iter()
            .map(|c| (c.key.as_str(), c.value.as_str()))
            .collect();
        assert_eq!(kv.get("analysis_id"), Some(&"task-9"));
        assert_eq!(kv.get("client_id"), Some(&"host-1"));
        assert_eq!(kv.get("source_file"), Some(&"cupti.json"));
        assert_eq!(kv.get("cat"), Some(&"cuda_kernel"));
        assert_eq!(kv.get("name"), Some(&"gemm"));
        assert_eq!(kv.get("ph"), Some(&"X"));
        assert_eq!(kv.get("pid"), Some(&"42"));
        assert_eq!(kv.get("tid"), Some(&"7"));
        assert_eq!(kv.get("ts_us"), Some(&"1500000"));
        assert_eq!(kv.get("dur_us"), Some(&"500"));
        assert!(kv.get("args_json").unwrap().contains("grid"));

        // Second entry has no dur / args
        let second = &v[1];
        let kv2: std::collections::HashMap<_, _> = second
            .contents
            .iter()
            .map(|c| (c.key.as_str(), c.value.as_str()))
            .collect();
        assert_eq!(kv2.get("ph"), Some(&"B"));
        assert!(kv2.get("dur_us").is_none());
        assert!(kv2.get("args_json").is_none());
    }

    #[test]
    fn parses_bare_array_shape() {
        let d = tempdir().unwrap();
        let body = r#"[{"ph":"X","cat":"c","name":"n","pid":1,"tid":2,"ts":0,"dur":1}]"#;
        write_file(d.path(), "kernel-event.json", body);
        let v = sls_events_from_dir(d.path(), "a", "c").unwrap();
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn skips_non_trace_files_and_malformed() {
        let d = tempdir().unwrap();
        write_file(d.path(), "OFFSET.json", r#"{"pid":"nope"}"#); // name doesn't match
        write_file(d.path(), "cupti.json", "not json at all");
        write_file(d.path(), "torch-profile.json", r#"{"traceEvents":[]}"#);
        let v = sls_events_from_dir(d.path(), "a", "c").unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn scans_multiple_trace_families() {
        let d = tempdir().unwrap();
        let body = r#"{"traceEvents":[{"ph":"X","cat":"c","name":"n","pid":1,"tid":1,"ts":1000000}]}"#;
        write_file(d.path(), "cupti.json", body);
        write_file(d.path(), "torch-profile.json", body);
        write_file(d.path(), "kernel-event.json", body);
        let v = sls_events_from_dir(d.path(), "a", "c").unwrap();
        assert_eq!(v.len(), 3);
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    fn writer_with(states: &[(i32, CollectorState)]) -> Writer {
        let mut w = Writer {
            pid_map: HashMap::new(),
            offset: HashMap::new(),
        };
        for (pid, state) in states {
            w.pid_map
                .entry(*pid)
                .or_insert_with(HashMap::new)
                .insert(CollectorName::Pyki, *state);
        }
        w
    }

    #[test]
    fn not_finished_while_any_pid_is_still_writing() {
        // The multi-pid data-loss regression: pid 1 has flushed, pid 2 has been
        // told to stop but has not written yet. Finalizing here would merge pid
        // 1 and silently discard pid 2's trace.
        let w = writer_with(&[
            (1, CollectorState::WrittingOver),
            (2, CollectorState::UnWritting),
        ]);
        assert!(!w.all_collectors_finished());
    }

    #[test]
    fn finished_once_every_pid_reports() {
        let w = writer_with(&[
            (1, CollectorState::WrittingOver),
            (2, CollectorState::WrittingOver),
        ]);
        assert!(w.all_collectors_finished());
    }

    #[test]
    fn failed_counts_as_terminal() {
        let w = writer_with(&[
            (1, CollectorState::WrittingOver),
            (2, CollectorState::Failed),
        ]);
        assert!(w.all_collectors_finished());
    }

    #[test]
    fn watchdog_salvage_unblocks_healthy_pids() {
        // A collector that dies without reporting must not strand the run:
        // marking it failed has to make the whole run finalizable.
        let mut w = writer_with(&[
            (1, CollectorState::WrittingOver),
            (2, CollectorState::Collecting),
        ]);
        assert!(!w.all_collectors_finished());

        let abandoned = w.mark_pending_as_failed();

        assert_eq!(abandoned, vec![(2, CollectorName::Pyki)]);
        assert!(w.all_collectors_finished());
        assert_eq!(w.pid_map[&1][&CollectorName::Pyki], CollectorState::WrittingOver);
    }
}
