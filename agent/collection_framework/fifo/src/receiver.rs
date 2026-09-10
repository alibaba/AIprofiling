use super::{log, FifoHandlerInfo, GenericResult, UpstreamMessage};
use crate::concat_strings;
use jzon::{object, JsonValue};
use pretty_hex::*;
use std::{
    collections::{HashMap, HashSet},
    fs::{read_to_string, File},
    io::{self, copy, BufReader, ErrorKind, Seek, SeekFrom, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "protocal_ascii")]
use super::cupti_parser::{self, BufferBreaker, BufferBreakerState};

#[cfg(feature = "protocal_gp")]
use super::bin_parser::{self, BinParser, BinParserState, GpType};

#[derive(Debug)]
struct TracingInfo {
    begin: f64,
    end: f64,
    json: JsonValue,
    name: String,
    output: PathBuf,
    buffer_path: PathBuf,
    proc_name: String,
    cpu_pid: HashSet<usize>,
    cpu_tid: HashSet<usize>,
    gpu_pid: HashSet<usize>,
    gpu_tid: HashSet<usize>,
    start_ts: usize,
    limit: usize,
    legacy_output: bool,
    pid_maps: Option<HashMap<u32, u32>>,
    pub missed: usize,
    pub total: usize,
    pub size: usize,
}

impl TracingInfo {
    pub fn new<P: AsRef<Path>>(
        name: &str,
        output: P,
        start_ts: usize,
        limit: usize,
        legacy_output: bool,
        pid_maps: Option<HashMap<u32, u32>>,
    ) -> Self {
        let proc_name = name
            .split('_')
            .last()
            .map(|p| format!("GPU kernel[{}]", p))
            .unwrap_or_else(|| "unknown".to_string())
            .trim()
            .to_string();

        let mut buffer = JsonValue::new_object();
        buffer["schemaVersion"] = JsonValue::from(1);
        buffer["distributedInfo"] = JsonValue::new_object();
        _ = buffer["distributedInfo"].insert("rank", JsonValue::from(""));

        buffer["deviceProperties"] = JsonValue::new_array();
        buffer["traceEvents"] = JsonValue::new_array();

        let ts_start = get_current_ts();
        let buffer_path = output
            .as_ref()
            .join(format!(".{}.{}.buffer", name, ts_start));
        if legacy_output {
            log::warn!("you are using legacy output, this may cause larger memory usage.");
        }
        Self {
            begin: std::f64::MAX,
            end: 0.0,
            json: buffer,
            name: name.to_string(),
            output: output.as_ref().to_path_buf(),
            buffer_path: buffer_path,
            proc_name: proc_name,
            cpu_pid: HashSet::new(),
            cpu_tid: HashSet::new(),
            gpu_pid: HashSet::new(),
            gpu_tid: HashSet::new(),
            start_ts: start_ts,
            limit: limit.saturating_sub(limit / 72),
            legacy_output: legacy_output,
            pid_maps: pid_maps,
            missed: 0,
            total: 0,
            size: 0,
        }
    }

    fn update_boundary(&mut self, ts: f64) {
        self.begin = self.begin.min(ts);
        self.end = self.end.max(ts);
    }

    fn cpu_pid_object(&self, pid: usize, ts: f64) -> (JsonValue, JsonValue, JsonValue) {
        (
            object! {
                "name": "process_name",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": 0,
                "args": {
                    "name": self.proc_name.clone()
                }
            },
            object! {
                "name": "process_labels",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": 0,
                "args": {
                    "labels": "CPU"
                }
            },
            object! {
                "name": "process_sort_index",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": 0,
                "args": {
                    "sort_index": pid
                }
            },
        )
    }
    fn update_cpu_pid(&mut self, pid: usize, ts: f64) {
        if self.cpu_pid.insert(pid) {
            let (process_name, process_labels, process_sort_index) = self.cpu_pid_object(pid, ts);
            if self.legacy_output {
                self.size += process_name.dump().len();
                _ = self.json["traceEvents"].push(process_name);

                self.size += process_labels.dump().len();
                _ = self.json["traceEvents"].push(process_labels);

                self.size += process_sort_index.dump().len();
                _ = self.json["traceEvents"].push(process_sort_index);
            } else {
                let buffer = concat_strings!(process_name, process_labels, process_sort_index; ",");
                self.wirte_buffer(buffer.as_bytes());
            }
        }
    }

    fn cpu_tid_object(&self, tid: usize, pid: usize, ts: f64) -> (JsonValue, JsonValue) {
        (
            object! {
                "name": "thread_name",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": tid,
                "args": {
                    "name": format!("thread {}", tid)
                }
            },
            object! {
                "name": "thread_sort_index",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": tid,
                "args": {
                    "sort_index": tid
                }
            },
        )
    }

    fn update_cpu_tid(&mut self, tid: usize, pid: usize, ts: f64) {
        if self.cpu_tid.insert(tid) {
            let (thread_name, thread_sort_index) = self.cpu_tid_object(tid, pid, ts);
            if self.legacy_output {
                self.size += thread_name.dump().len();
                _ = self.json["traceEvents"].push(thread_name);

                self.size += thread_sort_index.dump().len();
                _ = self.json["traceEvents"].push(thread_sort_index);
            } else {
                let buffer = concat_strings!(thread_name, thread_sort_index; ",");
                self.wirte_buffer(buffer.as_bytes());
            }
        }
    }

    fn gpu_pid_object(&self, pid: usize, ts: f64) -> (JsonValue, JsonValue, JsonValue) {
        (
            object! {
                "name": "process_name",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": 0,
                "args": {
                    "name": self.proc_name.clone()
                }
            },
            object! {
                "name": "process_labels",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": 0,
                "args": {
                    "labels": "GPU"
                }
            },
            object! {
                "name": "process_sort_index",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": 0,
                "args": {
                    "sort_index": std::usize::MAX - pid
                }
            },
        )
    }

    fn update_gpu_pid(&mut self, pid: usize, ts: f64) {
        if self.gpu_pid.insert(pid) {
            let (process_name, process_labels, process_sort_index) = self.gpu_pid_object(pid, ts);
            if self.legacy_output {
                self.size += process_name.dump().len();
                _ = self.json["traceEvents"].push(process_name);

                self.size += process_labels.dump().len();
                _ = self.json["traceEvents"].push(process_labels);

                self.size += process_sort_index.dump().len();
                _ = self.json["traceEvents"].push(process_sort_index);
            } else {
                let buffer = concat_strings!(process_name, process_labels, process_sort_index; ",");
                self.wirte_buffer(buffer.as_bytes());
            }
        }
    }

    fn gpu_tid_object(&self, tid: usize, pid: usize, ts: f64) -> (JsonValue, JsonValue) {
        (
            object! {
                "name": "thread_name",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": tid,
                "args": {
                    "name": format!("stream {}", tid)
                }
            },
            object! {
                "name": "thread_sort_index",
                "ph": "M",
                "ts": ts,
                "pid": pid,
                "tid": tid,
                "args": {
                    "sort_index": tid
                }
            },
        )
    }

    fn update_gpu_tid(&mut self, tid: usize, pid: usize, ts: f64) {
        if self.gpu_tid.insert(tid) {
            let (thread_name, thread_sort_index) = self.gpu_tid_object(tid, pid, ts);

            if self.legacy_output {
                self.size += thread_name.dump().len();
                _ = self.json["traceEvents"].push(thread_name);

                self.size += thread_sort_index.dump().len();
                _ = self.json["traceEvents"].push(thread_sort_index);
            } else {
                let buffer = concat_strings!(thread_name, thread_sort_index; ",");
                self.wirte_buffer(buffer.as_bytes());
            }
        }
    }

    pub fn push_event_cpu(
        &mut self,
        mut event: JsonValue,
        correlation_id: usize,
        flow_start: bool,
    ) {
        let len = event.dump().len();
        if self.limit > 0 && self.size + len > self.limit {
            self.missed += 1;
            return;
        }

        let ts = event["ts"].as_f64().unwrap_or(std::f64::NAN);
        self.update_boundary(ts);
        let pid = event["pid"].as_usize().unwrap_or_default();
        let pid = *self
            .pid_maps
            .as_ref()
            .and_then(|maps| maps.get(&(pid as u32)))
            .unwrap_or(&(pid as u32)) as usize;
        event["pid"] = JsonValue::Number(pid.into());
        self.update_cpu_pid(pid, ts);
        let tid = event["tid"].as_usize().unwrap_or_default();
        let tid = *self
            .pid_maps
            .as_ref()
            .and_then(|maps| maps.get(&(tid as u32)))
            .unwrap_or(&(tid as u32)) as usize;
        event["tid"] = JsonValue::Number(tid.into());
        self.update_cpu_tid(tid, pid, ts);
        let flow_event = if flow_start {
            object! {
                "name": "ac2g",
                "cat": "ac2g",
                "ph": "s",
                "ts": ts,
                "pid": pid,
                "tid": tid,
                "id": correlation_id
            }
        } else {
            object! {
                "name": "ac2g",
                "cat": "ac2g",
                "ph": "f",
                "ts": ts,
                "pid": pid,
                "tid": tid,
                "bp": "e",
                "id": correlation_id
            }
        };
        self.size += len + flow_event.dump().len();
        if self.legacy_output {
            _ = self.json["traceEvents"].push(event);
            _ = self.json["traceEvents"].push(flow_event);
        } else {
            let buffer = concat_strings!(event, flow_event; ",");
            self.wirte_buffer(buffer.as_bytes());
        }
    }

    pub fn push_event_gpu(&mut self, event: JsonValue, correlation_id: usize) {
        let len = event.dump().len();
        if self.limit > 0 && self.size + len > self.limit {
            self.missed += 1;
            return;
        }

        let ts = event["ts"].as_f64().unwrap_or(std::f64::NAN);
        self.update_boundary(ts);
        let pid = event["pid"].as_usize().unwrap_or_default();
        self.update_gpu_pid(pid, ts);
        let tid = event["tid"].as_usize().unwrap_or_default();
        self.update_gpu_tid(tid, pid, ts);

        // if event["cat"].as_str().filter(|&cat| cat == "kernel").is_some() {
        //     let device_id = pid;
        //     if let Some(args) = event["args"].as_object() {
        //         let total_grid = args["_total_grid"].as_i64().unwrap_or_else(||{
        //             log::warn!("total_grid is invalid, set to 1");
        //             1
        //         });
        //         let total_block = args["_total_block"].as_i64().unwrap_or_default();
        //         let props = &self.json["deviceProperties"][device_id];

        //         // log::info!("{}", props);
        //         let sm_count = props["multiProcessorCount"].as_i64().unwrap_or(- total_grid);
        //         let wrap_size = props["warpSize"].as_usize().unwrap_or(32);

        //         let blocks_per_sm = total_grid as f64 / sm_count as f64;
        //         let wrap_per_sm = blocks_per_sm * total_block as f64 / wrap_size as f64;

        //         _ = event[""]

        //         log::info!("device_id: {}, sm_count: {}, blocks_per_sm: {}, wrap_per_sm: {}", device_id, sm_count, blocks_per_sm, wrap_per_sm);

        //     }
        // }

        let ac2g_event = object! {
            "name": "ac2g",
            "cat": "ac2g",
            "ph": "f",
            "ts": ts,
            "pid": pid,
            "tid": tid,
            "bp": "e",
            "id": correlation_id
        };

        self.size += len + ac2g_event.dump().len();
        if self.legacy_output {
            _ = self.json["traceEvents"].push(event);
            _ = self.json["traceEvents"].push(ac2g_event);
        } else {
            let buffer = concat_strings!(event, ac2g_event; ",");
            self.wirte_buffer(buffer.as_bytes());
        }
    }

    pub fn push_props(&mut self, event: JsonValue) {
        _ = self.json["deviceProperties"].push(event);
    }

    pub fn push_metadata(&mut self, event: JsonValue) {
        let len = event.dump().len();
        if self.limit > 0 && self.size + len > self.limit {
            self.missed += 1;
            return;
        }
        if self.legacy_output {
            _ = self.json["traceEvents"].push(event);
        } else {
            let buffer = concat_strings!(event; ",");
            self.wirte_buffer(buffer.as_bytes());
        }
    }

    fn record_window_object(&self) -> (JsonValue, JsonValue) {
        (
            object! {
                "name": "Record Window Start",
                "ph": "i",
                "s": "g",
                "pid": "",
                "tid": "",
                "ts": self.begin
            },
            object! {
                "name": "Record Window End",
                "ph": "i",
                "s": "g",
                "pid": "",
                "tid": "",
                "ts": self.end
            },
        )
    }

    fn wirte_buffer(&self, buf: &[u8]) {
        if let Err(err) = append_buffer(&self.buffer_path, buf) {
            log::error!("write buffer failed: {}", err);
            log::trace!("buffer: {}", String::from_utf8_lossy(&buf));
        }
    }

    fn write_file(&mut self) -> GenericResult {
        log::info!("writing trace info {}", self.name);
        let file_name = format!("{}-{}.json", self.name, self.start_ts);
        let p = self.output.join(&file_name);
        let mut file = File::options().write(true).create(true).open(&p)?;
        let (begin, end) = self.record_window_object();
        if self.legacy_output {
            _ = self.json["traceEvents"].push(begin);
            _ = self.json["traceEvents"].push(end);
        } else {
            let buffer = concat_strings!(begin, end; "]}");
            self.wirte_buffer(buffer.as_bytes());
        }
        file.write_all(self.json.dump().as_bytes())?;
        file.flush()?;

        if !self.legacy_output {
            log::debug!("processing buffer {}", p.display());
            let len = file.metadata()?.len();
            let pos = len.saturating_sub(2);
            file.seek(SeekFrom::Start(pos))?;
            let mut buffer = BufReader::new(File::open(&self.buffer_path)?);
            copy(&mut buffer, &mut file)?;
            file.flush()?;
            if !log::enabled!(log::Level::DEBUG) {
                std::fs::remove_file(&self.buffer_path)?;
            }
        }

        log::info!(
            "Success write {}, total processed: {} metrics, dropped: {} metrics, size: {}",
            file_name,
            self.total,
            self.missed,
            self.size,
        );
        Ok(())
    }
}

// impl Drop for TracingInfo {
//     fn drop(&mut self) {
//         if let Err(err) = self.write_file() {
//             log::error!("write file failed: {}", err);
//         }
//     }
// }

fn append_buffer<P: AsRef<Path>>(path: P, buf: &[u8]) -> io::Result<()> {
    let mut file = File::options().append(true).create(true).open(path)?;
    file.write_all(buf)
}

fn get_current_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

// fn write_file(write_buffer: &mut HashMap<String, TracingInfo>, name: &str) -> GenericResult {
//     write_buffer.remove(name);
//     Ok(())
// }

#[cfg(feature = "protocal_ascii")]
fn parse_payload<P: AsRef<Path>>(
    write_buffer: &mut HashMap<String, TracingInfo>,
    output: P,
    id: usize,
    name: &str,
    payload: &str,
    info: broadcast::Sender<FifoHandlerInfo>,
    limit: usize,
) {
    let (cmd, args) = if let Some((cmd, args)) = payload.split_once(' ') {
        (cmd, args)
    } else {
        (payload, "")
    };
    match cmd {
        "cupti," => {
            log::trace!("{} cupti: {}", id, args);
            if args.eq("DeInitCuptiTrace") {
                if let Err(err) = write_file(write_buffer, &output, &name) {
                    log::error!("write file failed: {}", err);
                } else {
                    _ = info.send(FifoHandlerInfo::Finish(name.to_string()));
                }
            } else {
                write_buffer
                    .entry(name.to_string())
                    .and_modify(|e| parse_cupit(e, args))
                    .or_insert_with(|| {
                        let proc_name = name
                            .split('_')
                            .last()
                            .and_then(|p| read_to_string(format!("/proc/{}/comm", p)).ok())
                            .unwrap_or("unknown".to_string())
                            .trim()
                            .to_string();
                        let mut tracing_info = TracingInfo::new(proc_name, limit);
                        parse_cupit(&mut tracing_info, args);
                        tracing_info
                    });
            }
        }
        ".fin" => {
            log::debug!("{} fin: {}", id.clone(), args);
            if let Err(err) = write_file(write_buffer, &output, &name) {
                log::error!("write file failed: {}", err);
            }
            // _ = info.send(FifoHandlerInfo::Finish(name.to_string()));
            // if write_buffer.contains_key(name) {
            //     log::warn!("still got data in {}, not ready to fin", name);
            // }
            // TODO: delete fifo when fin
        }
        _ => {
            log::debug!("invalid input: {:?}", payload);
        }
    }
}

pub fn upstream_receiver(
    output: PathBuf,
    upstream: &mut mpsc::UnboundedReceiver<UpstreamMessage>,
    info: broadcast::Sender<FifoHandlerInfo>,
    name: String,
    start_ts: usize,
    limit: usize,
    legacy_output: bool,
    pid_maps: Option<HashMap<u32, u32>>,
) -> GenericResult {
    log::info!("upstream receiver for {} start", name);

    #[cfg(feature = "protocal_ascii")]
    let mut breaker_map: HashMap<String, BufferBreaker> = HashMap::new();

    #[cfg(feature = "protocal_gp")]
    let mut breaker = BinParser::default();

    #[cfg(feature = "protocal_gp")]
    let mut write_buffer = std::cell::Cell::new(TracingInfo::new(
        &name,
        &output,
        start_ts,
        limit,
        legacy_output,
        pid_maps,
    ));

    loop {
        match upstream.blocking_recv() {
            Some(UpstreamMessage::_Payload((id, name, payload))) => {
                log::debug!("controller got msg: {}-{}-{}", id, name, payload);
                #[cfg(feature = "protocal_ascii")]
                parse_payload(
                    &mut write_buffer,
                    &output,
                    id,
                    &name,
                    &payload,
                    info.clone(),
                    limit,
                );
            }
            Some(UpstreamMessage::Up((id, name))) => {
                log::debug!("controller got up: {}-{}", id, name);
                #[cfg(feature = "protocal_ascii")]
                breaker_map.insert(name.clone(), BufferBreaker::new());
                // #[cfg(feature = "protocal_gp")]
                // breaker_map.insert(name.clone(), BinParser::default());
                _ = info.send(FifoHandlerInfo::Ready(name));
            }
            #[cfg(feature = "protocal_ascii")]
            Some(UpstreamMessage::Buffer((id, name, buffer, _))) => {
                log::debug!("controller got buffer: {}-{}", id, name);
                if log::enabled!(log::Level::DEBUG) {
                    let file_name = format!("{}-{}.raw", name, id);
                    let p = &output.join(&file_name);
                    let mut file = File::options().append(true).create(true).open(&p)?;
                    file.write_all(&buffer)?;
                    file.flush()?;
                }
                if let Some(breaker) = breaker_map.get_mut(&name) {
                    breaker.reload(buffer);
                    while let Ok((line, state)) = breaker.next() {
                        match state {
                            BufferBreakerState::Readable => {
                                parse_payload(
                                    &mut write_buffer,
                                    &output,
                                    id,
                                    &name,
                                    &line,
                                    info.clone(),
                                    limit,
                                );
                            }
                            BufferBreakerState::Empty | BufferBreakerState::Reload => {
                                break;
                            }
                            BufferBreakerState::Continue => {}
                        }
                    }
                }
            }
            #[cfg(feature = "protocal_gp")]
            Some(UpstreamMessage::Buffer((id, name, buffer, size))) => {
                log::trace!("controller got buffer: {}-{}", id, name);
                if log::enabled!(log::Level::DEBUG) {
                    let file_name = format!("{}-{}.raw", name, id);
                    let p = &output.join(&file_name);
                    let mut file = File::options().append(true).create(true).open(&p).unwrap();
                    file.write_all(format!("currunt_size:0x{:016x}<", size).as_bytes())
                        .unwrap();
                    file.write_all(format!("remain_block:0x{:016x}<", upstream.len()).as_bytes())
                        .unwrap();
                    file.write_all(&buffer).unwrap();
                    file.flush().unwrap();
                }
                if breaker.reload(buffer, size).is_ok() {
                    while let Ok(BinParserState::Readable((ty, contents))) = breaker.next() {
                        log::trace!("got {:?}", ty);
                        if ty == GpType::GpExit {
                            log::info!("{}-{} got exit", id, name);
                            if let Err(err) = write_buffer.get_mut().write_file() {
                                log::error!("write file failed: {}", err);
                            }
                            _ = info.send(FifoHandlerInfo::Finish(name.to_string()));
                            break;
                        }
                        let info = write_buffer.get_mut();
                        parse_cupit(info, &contents);
                    }
                } else {
                    log::error!("load bad buffer:{}", size);
                    break;
                }
            }
            Some(UpstreamMessage::Finish((id, name))) => {
                log::debug!("controller got fin: {}-{}", id, name);
                #[cfg(feature = "protocal_ascii")]
                if let Some(breaker) = breaker_map.remove(&name) {
                    let line = breaker.finish();
                    let count = breaker.count;
                    let block = breaker.block;
                    log::debug!("{} input {} blocks, {} cmds", name, block, count - block);
                    if !line.is_empty() {
                        parse_payload(
                            &mut write_buffer,
                            &output,
                            id,
                            &name,
                            &line,
                            info.clone(),
                            limit,
                        );
                    }
                    if let Err(err) = write_file(&mut write_buffer, &output, &name) {
                        log::error!("write file failed: {}", err);
                    }
                    let mut fifo_map = fifo_map.blocking_lock();
                    if let Err(err) = fifo_map.remove(&name) {
                        log::info!("remove fifo failed: {}", err);
                    }
                    _ = info.send(FifoHandlerInfo::Finish(name.to_string()));
                }
            }
            None => break,
        }
    }
    log::debug!("upstream receiver end");
    Ok(())
}

#[cfg(feature = "protocal_ascii")]
fn parse_cupit(info: &mut TracingInfo, msg: &str) {
    match cupti_parser::parse_msg(msg, 0) {
        Ok((_, (j, t))) => {
            log::trace!("cupti parse: {}, type:{:?}", j, t);
            match t {
                cupti_parser::MsgType::EventCPU((correlation_id, flow_start)) => {
                    _ = info.push_event_cpu(JsonValue::from(j), correlation_id, flow_start)
                }
                cupti_parser::MsgType::EventGPU(correlation_id) => {
                    _ = info.push_event_gpu(JsonValue::from(j), correlation_id)
                }
                cupti_parser::MsgType::Props => info.push_props(JsonValue::from(j)),
            }
            info.total += 1;
        }
        Err(err) => {
            // no need to know the error detail unless debug
            log::debug!("cupti parse error: {}", err);
            log::trace!("orinal msg: {}", msg);
            info.missed += 1;
            info.total += 1;
        }
    }
}

#[cfg(feature = "protocal_gp")]
fn parse_cupit(info: &mut TracingInfo, msg: &Vec<u8>) {
    match bin_parser::parse_msg(msg, info.start_ts as u64) {
        Ok((j, t)) => {
            log::trace!("cupti parse: {}, type:{:?}", j, t);
            match t {
                bin_parser::MsgType::EventCPU((correlation_id, flow_start)) => {
                    _ = info.push_event_cpu(JsonValue::from(j), correlation_id, flow_start)
                }
                bin_parser::MsgType::EventGPU(correlation_id) => {
                    _ = info.push_event_gpu(JsonValue::from(j), correlation_id)
                }
                bin_parser::MsgType::Props => info.push_props(JsonValue::from(j)),
                bin_parser::MsgType::Metadata => info.push_metadata(JsonValue::from(j)),
            }
            info.total += 1;
        }
        Err(err) => {
            match err.kind() {
                ErrorKind::InvalidData | ErrorKind::Unsupported => {
                    log::trace!(
                        "cupti parse warning: {}\norignal msg: \n{:?}",
                        err,
                        msg.hex_dump()
                    );
                }
                _ => {
                    // no need to know the error detail unless debug
                    log::debug!("cupti parse error: {}", err);
                    log::trace!("error {}: orignal msg:\n{:?}", err, msg.hex_dump());
                }
            }
            info.missed += 1;
            info.total += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;
    use tracing_subscriber::{
        filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
    };
    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            let _ = tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer())
                .with(
                    EnvFilter::builder()
                        .with_default_directive(LevelFilter::INFO.into())
                        .from_env_lossy(),
                )
                .try_init();
        });
    }

    #[test]
    fn test_json_eq() {
        init();
        let a = object! {
            "name": "Record Window End",
            "ph": "i",
            "s": "g",
            "pid": "",
            "tid": "",
            "ts": "1234"
        };
        assert_eq!(a.dump(), a.to_string());
    }

    #[test]
    #[cfg(feature = "protocal_ascii")]
    fn test_parse_cupti() {
        init();
        let mut write_buffer: HashMap<String, TracingInfo> = HashMap::new();

        let vec_contents = vec![
            r#"RUNTIME, 1725936353373055726, 41583213, "cudaDeviceReset_v3020", cbid 164, processId 3872819, threadId 2642882560, correlationId 957"#,
            r#"CONCURRENT_KERNEL, 1725936353518871674, 4544, "_Z14VectorSubtractPKiS0_Pii", correlationId 983, grid [ 196, 1, 1 ], block [ 256, 1, 1 ], cluster [ 0, 0, 0 ], sharedMemory (static 0, dynamic 0), deviceId 3, contextId 4, streamId 49, graphId 0, graphNodeId 0, channelId 0, channelType COMPUTE"#,
            r#"MEMCPY, 1725936353518389600, 17632, size 200000, "DtoH", srcKind DEVICE, dstKind PAGEABLE, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMCPY"#,
        ];

        for contents in vec_contents {
            write_buffer
                .entry("k".to_string())
                .and_modify(|e| parse_cupit(e, &contents))
                .or_insert_with(|| {
                    let mut tracing_info = TracingInfo::new("python".to_string(), 0);
                    parse_cupit(&mut tracing_info, &contents);
                    tracing_info
                });
        }
        for (k, v) in write_buffer {
            println!("{}: {:?}", k, v);
        }
    }
}
