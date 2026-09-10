use crate::include_header;
use jzon::JsonValue;
use nom::{
    bytes::complete::{tag, take_until},
    character::complete::char,
    sequence::delimited,
    IResult, ParseTo,
};
use std::{
    io::{BufRead, Cursor},
    str::FromStr,
};

use super::GenericErr;
use crate::TILE_SIZE;
include_header!("cupti_runtime_cbid");

// use tracing as log;

#[derive(Debug)]
pub enum MsgType {
    EventCPU((usize, bool)),
    EventGPU(usize),
    Props,
}

fn flow_start(cbid: u32) -> bool {
    cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaLaunchKernel_v7000 as u32 ||
      (cbid >= CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaMemcpy_v3020 as u32 &&
       cbid <= CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaMemset2DAsync_v3020 as u32)  ||
      cbid ==
          CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaLaunchCooperativeKernel_v9000 as u32 ||
      cbid ==
          CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaLaunchCooperativeKernelMultiDevice_v9000 as u32 ||
      cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaGraphLaunch_v10000 as u32 ||
      cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaStreamSynchronize_v3020 as u32 ||
      cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaDeviceSynchronize_v3020 as u32 ||
      cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaStreamWaitEvent_v3020 as u32
}

// cupti, MEMCPY, 1725936353518389600, 17632, size 200000, "DtoH", srcKind DEVICE, dstKind PAGEABLE, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMCPY
// cupti, MEMSET, 1725936353518389600, 17632, size 200000, "DtoH", srcKind DEVICE, dstKind PAGEABLE, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMCPY

// cupti, CONCURRENT_KERNEL, 1725936353518871674, 4544, "_Z14VectorSubtractPKiS0_Pii", correlationId 983, grid [ 196, 1, 1 ], block [ 256, 1, 1 ], cluster [ 0, 0, 0 ], sharedMemory (static 0, dynamic 0), deviceId 3, contextId 4, streamId 49, graphId 0, graphNodeId 0, channelId 0, channelType COMPUTE

// cupti, RUNTIME, 1725936353373055726, 41583213, "cudaDeviceReset_v3020", cbid 164, processId 3872819, threadId 2642882560, correlationId 957

// cupti, DRIVER, 1722930018920677001, 336, "cuDeviceGetUuid", cbid 482, processId 3131337, threadId 429703168, correlationId 493

pub fn parse_msg(input: &str, ts_off: u64) -> IResult<&str, (JsonValue, MsgType)> {
    // let (input, _) = tag("cupti,")(input)?;
    let (input, format) = take_until(" ")(input)?;

    match format {
        "RUNTIME," => parse_runtime(input.trim(), ts_off),
        "CONCURRENT_KERNEL," => parse_concurrent_kernel(input.trim(), ts_off),
        "MEMCPY," => parse_memcpy(input.trim(), ts_off),
        "MEMSET," => parse_memset(input.trim(), ts_off),
        "DRIVER," => parse_driver(input.trim(), ts_off),
        "DEVICE," => parse_device(input.trim(), ts_off),
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        ))),
    }
}

fn new_trace_evnet(cat: &str) -> JsonValue {
    let mut buf = JsonValue::new_object();

    buf["ph"] = "X".into();
    buf["cat"] = JsonValue::from(cat);

    buf["args"] = JsonValue::new_object();
    _ = buf["args"].insert("External id", JsonValue::from(0));

    buf
}

fn parse_time_and_duration(input: &str, ts_off: u64) -> IResult<&str, (f64, f64)> {
    let (input, ts_str) = take_until(" ")(input)?;
    let ts: u64 = ts_str
        .trim_end_matches(',')
        .parse_to()
        .ok_or(nom::Err::Error(nom::error::Error::new(
            ts_str,
            nom::error::ErrorKind::Fail,
        )))?;

    // filter out the ts equal to 0 or std::u64::MAX, that means the ts data is corrupted
    if ts == 0 || ts == std::u64::MAX {
        return Err(nom::Err::Error(nom::error::Error::new(
            ts_str,
            nom::error::ErrorKind::Verify,
        )));
    }

    let (input, dur_str) = take_until(" ")(input.trim())?;
    let dur: u64 = dur_str
        .trim_end_matches(',')
        .parse_to()
        .ok_or(nom::Err::Error(nom::error::Error::new(
            dur_str,
            nom::error::ErrorKind::Verify,
        )))?;
    Ok((
        input,
        (
            ts.saturating_sub(ts_off) as f64 / 1000.0,
            dur as f64 / 1000.0,
        ),
    ))
}

fn parse_name(input: &str) -> IResult<&str, &str> {
    delimited(char('"'), take_until("\""), tag("\", "))(input.trim())
}

fn parse_segment<'a>(input: &'a str, t: &'static str) -> IResult<&'a str, &'a str> {
    delimited(tag(t), take_until(","), tag(", "))(input)
}

fn parse_segment_number<'a, R: FromStr>(input: &'a str, t: &'static str) -> IResult<&'a str, R> {
    let (res, number_str) = delimited(tag(t), take_until(","), tag(", "))(input)?;
    let number: R = number_str
        .parse_to()
        .ok_or(nom::Err::Error(nom::error::Error::new(
            number_str,
            nom::error::ErrorKind::Verify,
        )))?;
    Ok((res, number))
}

// fn parse_final<'a>(input: &'a str, t: &'static str) -> IResult<&'a str, &'a str> {
//     let (item, res) = tag(t)(input)?;
//     Ok((res, item))
// }

fn parse_final_number<'a, R: FromStr>(input: &'a str, t: &'static str) -> IResult<&'a str, R> {
    let (number_str, res) = tag(t)(input)?;
    let number: R = number_str
        .parse_to()
        .ok_or(nom::Err::Error(nom::error::Error::new(
            number_str,
            nom::error::ErrorKind::Verify,
        )))?;
    Ok((res, number))
}

fn parse_block<'a>(input: &'a str, t: &'static str) -> IResult<&'a str, &'a str> {
    delimited(tag(t), take_until("]"), tag("], "))(input)
}

fn is_block_listed_runtime_cbid(cbid: u32) -> bool {
    // Some CUDA calls that are very frequent and also not very interesting.
    // Filter these out to reduce trace size.
    if cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaGetDevice_v3020 as u32 ||
        cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaSetDevice_v3020 as u32 ||
        cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaGetLastError_v3020 as u32 ||
        // Support cudaEventRecord and cudaEventSynchronize, revisit if others are
        // needed
        cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaEventCreate_v3020 as u32 ||
        cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaEventCreateWithFlags_v3020 as u32 ||
        cbid == CUpti_runtime_api_trace_cbid::CUPTI_RUNTIME_TRACE_CBID_cudaEventDestroy_v3020 as u32
    {
        return true;
    }

    return false;
}

// 1725936353373055726, 41583213, "cudaDeviceReset_v3020", cbid 164, processId 3872819, threadId 2642882560, correlationId 957
fn parse_runtime(input: &str, ts_off: u64) -> IResult<&str, (JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("cuda_runtime");

    let (input, (ts, dur)) = parse_time_and_duration(input, ts_off)?;
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    let (input, name) = parse_name(input)?;
    let name = name.split('_').next().unwrap_or(name);
    buf["name"] = JsonValue::from(name);

    // let (input, cbid) = delimited(tag("cbid "), take_until(","), tag(", "))(input)?;
    let (input, cbid) = parse_segment_number::<usize>(input, "cbid ")?;
    _ = buf["args"].insert("cbid", JsonValue::from(cbid));
    if is_block_listed_runtime_cbid(cbid as u32) {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }

    let (input, process_id) = parse_segment_number::<usize>(input, "processId ")?;
    buf["pid"] = JsonValue::from(process_id);

    let (input, thread_id) = parse_segment_number::<usize>(input, "threadId ")?;
    buf["tid"] = JsonValue::from(thread_id);

    let (input, correlation_id) = parse_final_number::<usize>(input, "correlationId ")?;
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    Ok((
        input,
        (
            buf,
            MsgType::EventCPU((correlation_id, flow_start(cbid as u32))),
        ),
    ))
}

//CONCURRENT_KERNEL
// 1725936353518871674, 4544, "_Z14VectorSubtractPKiS0_Pii", correlationId 983, grid [ 196, 1, 1 ], block [ 256, 1, 1 ], cluster [ 0, 0, 0 ], sharedMemory (static 0, dynamic 0), deviceId 3, contextId 4, streamId 49, graphId 0, graphNodeId 0, channelId 0, channelType COMPUTE
fn parse_concurrent_kernel(input: &str, ts_off: u64) -> IResult<&str, (JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("kernel");

    let (input, (ts, dur)) = parse_time_and_duration(input, ts_off)?;
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    let (input, name) = parse_name(input)?;
    let name = cpp_demangle::Symbol::new(name)
        .and_then(|sym| Ok(sym.to_string()))
        .unwrap_or(name.to_string());
    buf["name"] = JsonValue::from(name);

    let (input, correlation_id) = parse_segment_number::<usize>(input, "correlationId ")?;
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    let (input, grid) = parse_block(input, "grid [")?;
    _ = buf["args"].insert("grid", JsonValue::from(format!("[{}]", grid.trim())));

    let (input, block) = parse_block(input, "block [")?;
    _ = buf["args"].insert("block", JsonValue::from(format!("[{}]", block.trim())));

    let (input, _cluster) = parse_block(input, "cluster [")?;
    // _ = buf["args"].insert("cluster", JsonValue::from(cluster.trim()));

    let (input, shared_memory) =
        delimited(tag("sharedMemory ("), take_until(")"), tag("), "))(input)?;
    let (left, static_shared_memory) = parse_segment_number::<usize>(shared_memory, "static ")?;
    let (_, dynamic_shared_memory) = parse_final_number::<usize>(left, "dynamic ")?;
    _ = buf["args"].insert(
        "shared memory",
        JsonValue::from(dynamic_shared_memory + static_shared_memory),
    );

    let (input, device_id) = parse_segment_number::<usize>(input, "deviceId ")?;
    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));

    let (input, context_id) = parse_segment_number::<usize>(input, "contextId ")?;
    _ = buf["args"].insert("context", JsonValue::from(context_id));

    let (input, stream_id) = parse_segment_number::<usize>(input, "streamId ")?;
    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));

    // let (input, graph_id) = parse_segment_number::<usize>(input, "graphId ")?;
    // _ = buf["args"].insert("graph", JsonValue::from(graph_id));

    // let (input, graph_node_id) = parse_segment_number::<usize>(input, "graphNodeId ")?;
    // _ = buf["args"].insert("graph node", JsonValue::from(graph_node_id));

    // let (input, channel_id) = parse_segment_number::<usize>(input, "channelId ")?;
    // _ = buf["args"].insert("channel", JsonValue::from(channel_id));

    Ok((input, (buf, MsgType::EventGPU(correlation_id))))
}

// 1725936353518389600, 17632, size 200000, "DtoH", srcKind DEVICE, dstKind PAGEABLE, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMCPY
fn parse_memcpy<'a>(input: &'a str, ts_off: u64) -> IResult<&'a str, (JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("gpu_memcpy");

    let (input, (ts, dur)) = parse_time_and_duration(input, ts_off)?;
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    let (input, size) = parse_segment_number::<usize>(input.trim(), "size ")?;
    _ = buf["args"].insert("bytes", JsonValue::from(size));

    let (input, name) = parse_name(input)?;
    let (input, src_kind) = parse_segment(input, "srcKind ")?;
    let (input, dst_kind) = parse_segment(input, "dstKind ")?;
    let src_kind: String = src_kind
        .chars()
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    let dst_kind: String = dst_kind
        .chars()
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    buf["name"] = JsonValue::from(format!("Memcpy {} ({} -> {})", name, src_kind, dst_kind));

    let (input, correlation_id) = parse_segment_number::<usize>(input, "correlationId ")?;
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    let (input, device_id) = parse_segment_number::<usize>(input, "deviceId ")?;
    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));

    let (input, context_id) = parse_segment_number::<usize>(input, "contextId ")?;
    _ = buf["args"].insert("context", JsonValue::from(context_id));

    let (input, stream_id) = parse_segment_number::<usize>(input, "streamId ")?;
    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));

    // let (input, graph_id) = parse_segment_number::<usize>(input, "graphId ")?;
    // _ = buf["args"].insert("graph", JsonValue::from(graph_id));

    // let (input, graph_node_id) = parse_segment_number::<usize>(input, "graphNodeId ")?;
    // _ = buf["args"].insert("graph node", JsonValue::from(graph_node_id));

    // let (input, channel_id) = parse_segment_number::<usize>(input, "channelId ")?;
    // _ = buf["args"].insert("channel", JsonValue::from(channel_id));

    Ok((input, (buf, MsgType::EventGPU(correlation_id))))
}

// r#"cupti, MEMSET, 1725936353518389600, 17632, value 888, size 200000, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMSET"#
fn parse_memset(input: &str, ts_off: u64) -> IResult<&str, (JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("gpu_memset");

    let (input, (ts, dur)) = parse_time_and_duration(input, ts_off)?;
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    let (input, _value) = parse_segment_number::<usize>(input.trim(), "value ")?;
    // _ = buf["value"] = JsonValue::from(_value);

    let (input, size) = parse_segment_number::<usize>(input.trim(), "size ")?;
    _ = buf["args"].insert("bytes", JsonValue::from(size));

    buf["name"] = JsonValue::from(format!("Memset (Device)"));

    let (input, correlation_id) = parse_segment_number::<usize>(input, "correlationId ")?;
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    let (input, device_id) = parse_segment_number::<usize>(input, "deviceId ")?;
    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));

    let (input, context_id) = parse_segment_number::<usize>(input, "contextId ")?;
    _ = buf["args"].insert("context", JsonValue::from(context_id));

    let (input, stream_id) = parse_segment_number::<usize>(input, "streamId ")?;
    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));

    // let (input, graph_id) = parse_segment_number::<usize>(input, "graphId ")?;
    // _ = buf["args"].insert("graph", JsonValue::from(graph_id));

    // let (input, graph_node_id) = parse_segment_number::<usize>(input, "graphNodeId ")?;
    // _ = buf["args"].insert("graph node", JsonValue::from(graph_node_id));

    // let (input, channel_id) = parse_segment_number::<usize>(input, "channelId ")?;
    // _ = buf["args"].insert("channel", JsonValue::from(channel_id));

    Ok((input, (buf, MsgType::EventGPU(correlation_id))))
}

fn parse_driver(input: &str, ts_off: u64) -> IResult<&str, (JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("cuda_driver");

    let (input, (ts, dur)) = parse_time_and_duration(input, ts_off)?;
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    let (input, name) = parse_name(input)?;
    buf["name"] = JsonValue::from(name);

    let (input, cbid) = parse_segment_number::<usize>(input, "cbid ")?;
    _ = buf["args"].insert("cbid", JsonValue::from(cbid));

    //CUPTI_DRIVER_TRACE_CBID_cuLaunchKernel = 307
    if cbid != 307 {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Verify,
        )));
    }

    let (input, process_id) = parse_segment_number::<usize>(input, "processId ")?;
    buf["pid"] = JsonValue::from(process_id);

    let (input, thread_id) = parse_segment_number::<usize>(input, "threadId ")?;
    buf["tid"] = JsonValue::from(thread_id);

    let (input, correlation_id) = parse_final_number::<usize>(input, "correlationId ")?;
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    Ok((
        input,
        (
            buf,
            MsgType::EventCPU((correlation_id, flow_start(cbid as u32))),
        ),
    ))
}

// %u, %s, %llu, %u, %u, %u, %u, %u, %u, %u, %u, %u
// "id": =
// "name": =
// "totalGlobalMem": =
// "computeMajor": =
// "computeMinor": =
// "maxThreadsPerBlock": =
// "maxThreadsPerMultiprocessor": = maxWarpsPerMultiprocessor * 32
// "regsPerBlock": =
// "regsPerMultiprocessor": =
// "warpSize": 32
// "sharedMemPerBlock": =
// "sharedMemPerMultiprocessor": =
// "numSms": =
// "sharedMemPerBlockOptin": =

fn parse_comma_number<R: FromStr>(input: &str) -> IResult<&str, R> {
    let (input, number_str) = take_until(",")(input)?;
    let (input, _) = take_until(" ")(input)?;

    let number: R = number_str
        .parse_to()
        .ok_or(nom::Err::Error(nom::error::Error::new(
            number_str,
            nom::error::ErrorKind::Verify,
        )))?;
    Ok((input.trim(), number))
}

// i, props.name, props.totalGlobalMem, props.major, props.minor,
// props.maxThreadsPerBlock, props.maxThreadsPerMultiProcessor,
// props.regsPerBlock, props.warpSize, props.sharedMemPerBlock,
// props.multiProcessorCount, props.regsPerMultiprocessor,
// props.sharedMemPerBlockOptin, props.sharedMemPerMultiprocessor

fn parse_device(input: &str, _ts_off: u64) -> IResult<&str, (JsonValue, MsgType)> {
    let mut buf = JsonValue::new_object();

    let (input, id) = parse_comma_number::<usize>(input.trim())?;
    buf["id"] = JsonValue::from(id);

    let (input, name) = parse_name(input)?;
    buf["name"] = JsonValue::from(name);

    let (input, total_global_mem) = parse_comma_number::<u64>(input)?;
    buf["totalGlobalMem"] = JsonValue::from(total_global_mem);

    let (input, compute_major) = parse_comma_number::<usize>(input)?;
    buf["computeMajor"] = JsonValue::from(compute_major);

    let (input, compute_minor) = parse_comma_number::<usize>(input)?;
    buf["computeMinor"] = JsonValue::from(compute_minor);

    let (input, max_threads_per_block) = parse_comma_number::<usize>(input)?;
    buf["maxThreadsPerBlock"] = JsonValue::from(max_threads_per_block);

    let (input, max_threads_per_multiprocessor) = parse_comma_number::<usize>(input)?;
    buf["maxThreadsPerMultiprocessor"] = JsonValue::from(max_threads_per_multiprocessor);

    let (input, regs_per_block) = parse_comma_number::<usize>(input)?;
    buf["regsPerBlock"] = JsonValue::from(regs_per_block);

    let (input, wrap_size) = parse_comma_number::<usize>(input)?;
    buf["warpSize"] = JsonValue::from(wrap_size);

    let (input, shared_mem_per_block) = parse_comma_number::<usize>(input)?;
    buf["sharedMemPerBlock"] = JsonValue::from(shared_mem_per_block);

    let (input, multiprocessor_count) = parse_comma_number::<usize>(input)?;
    buf["multiProcessorCount"] = JsonValue::from(multiprocessor_count);

    let (input, regs_per_multiprocessor) = parse_comma_number::<usize>(input)?;
    buf["regsPerMultiprocessor"] = JsonValue::from(regs_per_multiprocessor);

    let (input, shared_mem_per_blockoption) = parse_comma_number::<usize>(input)?;
    buf["sharedMemPerBlockOptin"] = JsonValue::from(shared_mem_per_blockoption);

    // let (input, shared_mem_per_multiprocessor) = parse_comma_number::<usize>(input)?;
    // buf["sharedMemPerMultiprocessor"] = JsonValue::from(shared_mem_per_multiprocessor);

    let num_sms: usize = input
        .parse_to()
        .ok_or(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Verify,
        )))?;

    // let (input, num_sms) = parse_comma_number(input)?;
    buf["numSms"] = JsonValue::from(num_sms);
    buf["sharedMemPerMultiprocessor"] = JsonValue::from(num_sms);

    // log::info!("info: {}", info);
    Ok((input, (buf, MsgType::Props)))
}

#[derive(Debug, Clone)]
pub enum BufferBreakerState {
    Empty,
    Readable,
    Reload,
    Continue,
}

#[derive(Debug)]
pub struct BufferBreaker {
    last: String,
    state: BufferBreakerState,
    cursor: Cursor<[u8; TILE_SIZE]>,
    pub count: usize,
    pub block: usize,
}

impl BufferBreaker {
    pub fn new() -> Self {
        Self {
            last: String::new(),
            state: BufferBreakerState::Empty,
            cursor: Cursor::new([0; TILE_SIZE]),
            count: 0,
            block: 0,
        }
    }

    pub fn next(&mut self) -> Result<(String, BufferBreakerState), GenericErr> {
        let mut line = String::new();
        match self.state {
            // return empty when buffer and last are both empty
            BufferBreakerState::Empty => {
                self.state = BufferBreakerState::Empty;
            }

            // readable when buffer is not empty, and a cursor will run thought buffer,
            // return every line and set status to readable until cursor reach buffer end.
            BufferBreakerState::Readable => {
                self.cursor.read_line(&mut line)?;
                self.count += 1;
                if line.strip_suffix('\n').is_none() {
                    self.state = BufferBreakerState::Reload;
                    self.last = std::mem::take(&mut line);
                    self.block += 1;
                }
            }

            BufferBreakerState::Continue => {
                self.cursor.read_line(&mut line)?;
                let last = std::mem::take(&mut self.last);
                line = last.trim_matches('\0').to_string() + &line;
                self.state = BufferBreakerState::Readable;
                self.count += 1;
            }

            BufferBreakerState::Reload => {
                self.state = BufferBreakerState::Reload;
            }
        }
        Ok((line.trim_ascii().to_string(), self.state.clone()))
    }

    pub fn finish(&self) -> String {
        self.last.trim_matches('\0').to_string()
    }

    pub fn reload(&mut self, buf: [u8; TILE_SIZE]) {
        self.cursor = Cursor::new(buf);
        if self.last.is_empty() {
            self.state = BufferBreakerState::Readable
        } else {
            self.state = BufferBreakerState::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jzon::object;
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
    fn test_cupti_parse_runtime() {
        init();
        let contents = r#"RUNTIME, 1725936353373055726, 41583213, "cudaDeviceReset_v3020", cbid 164, processId 3872819, threadId 2642882560, correlationId 957"#;
        let (_, (buf, _)) = parse_msg(contents, 1).unwrap();
        let expect = object! {
            "ph":"X",
            "cat":"cuda_runtime",
            "args": {
                "External id":0,
                "cbid":164,
                "correlation":957
            },
            "ts":1725936353373055.725,
            "dur":41583.213,
            "name":"cudaDeviceReset",
            "pid":3872819,
            "tid":2642882560 as usize
        };
        assert_eq!(buf, expect);
    }

    #[test]
    fn test_cupti_parse_kernel() {
        init();
        let contents = r#"CONCURRENT_KERNEL, 1725936353518871674, 4544, "_Z14VectorSubtractPKiS0_Pii", correlationId 983, grid [ 196, 1, 1 ], block [ 256, 1, 1 ], cluster [ 0, 0, 0 ], sharedMemory (static 0, dynamic 0), deviceId 3, contextId 4, streamId 49, graphId 0, graphNodeId 0, channelId 0, channelType COMPUTE"#;
        let (_res, (buf, _)) = parse_msg(contents, 0).unwrap();
        let expect = object! {
            "ph":"X",
            "cat":"kernel",
            "args": {
                "External id":0,
                "correlation":983,
                "grid":"[196, 1, 1]",
                "block":"[256, 1, 1]",
                // "cluster":"0, 0, 0",
                "shared memory":0,
                "device":3,
                "context":4,
                "stream":49,
                // "graph":0,
                // "graph node":0,
                // "channel":0
            },
            "ts":1725936353518871.6,
            "dur":4.544,
            "name":"VectorSubtract(int const*, int const*, int*, int)",
            "pid":3,
            "tid":49
        };
        assert_eq!(buf, expect);
        // println!(">>>{}<<<", _res);
        // println!("{}", buf);
    }

    #[test]
    fn test_cupti_parse_memcpy() {
        init();
        let contents = r#"MEMCPY, 1725936353518389600, 17632, size 200000, "DtoH", srcKind DEVICE, dstKind PAGEABLE, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMCPY"#;
        let (_res, (buf, _)) = parse_msg(contents, 1).unwrap();
        let expect = object! {
            "ph":"X",
            "cat":"gpu_memcpy",
            "args":{
                "External id":0,
                "bytes":200000,
                "correlation":970,
                "device":3,
                "context":4,
                "stream":40,
                // "graph":0,
                // "graph node":0,
                // "channel":2
            },
            "ts":1725936353518389.599,
            "dur":17.632,
            "name":"Memcpy DtoH (Device -> Pageable)",
            "pid":3,
            "tid":40
        };
        assert_eq!(buf, expect);
        // println!(">>>{}<<<", _res);
        // println!("{}", buf);
    }

    #[test]
    fn test_cupti_parse_memset() {
        init();
        let contents = r#"MEMSET, 1725936353518389600, 17632, value 888, size 200000, correlationId 970, deviceId 3, contextId 4, streamId 40, graphId 0, graphNodeId 0, channelId 2, channelType ASYNC_MEMSET"#;
        let (_res, (buf, _)) = parse_msg(contents, 1).unwrap();
        let expect = object! {
            "ph":"X",
            "cat":"gpu_memset",
            "args":{
                "External id":0,
                "bytes":200000,
                "correlation":970,
                "device":3,
                "context":4,
                "stream":40,
                // "graph":0,
                // "graph node":0,
                // "channel":2
            },
            "ts":1725936353518389.599,
            "dur":17.632,
            "name":"Memset (Device)",
            "pid":3,
            "tid":40
        };
        assert_eq!(buf, expect);
        // println!(">>>{}<<<", _res);
        // println!("{}", buf);
    }

    #[test]
    fn test_cupti_parse_driver() {
        init();
        let contents = r#"DRIVER, 1722930018920677001, 336, "cuDeviceGetUuid", cbid 307, processId 3131337, threadId 429703168, correlationId 493"#;
        let (_res, (buf, _)) = parse_msg(contents, 0).unwrap();
        let expect = object! {
            "ph":"X",
            "cat":"cuda_driver",
            "args":{
                "External id":0,
                "cbid":307,
                "correlation":493
            },
            "ts":1722930018920677.001 ,
            "dur":0.336,
            "name":"cuDeviceGetUuid",
            "pid":3131337,
            "tid":429703168
        };
        assert_eq!(buf, expect);
        // println!(">>>{}<<<", _res);
        // println!("{}", buf);
    }

    // %u, %s, %llu, %u, %u, %u, %u, %u, %u, %u, %u, %u
    #[test]
    fn test_cupti_parse_device() {
        init();
        let contents = r#"DEVICE, 1, "cuDeviceGetUuid", 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13"#;
        let (_res, (buf, _)) = parse_msg(contents, 0).unwrap();
        // let expect = object! {
        //     "id":1,
        //     "name":"cuDeviceGetUuid",
        //     "totalGlobalMem":2,
        //     "computeMajor":3,
        //     "computeMinor":4,
        //     "maxThreadsPerBlock":5,
        //     "maxThreadsPerMultiprocessor":192,
        //     "regsPerBlock":7,
        //     "regsPerMultiprocessor":8,
        //     "warpSize":32,
        //     "sharedMemPerBlock":9,
        //     "sharedMemPerMultiprocessor":10,
        //     "sharedMemPerBlockOptin":10,
        //     "numSms":11
        // };

        let expect = object! {
            "id":1,
            "name":"cuDeviceGetUuid",
            "totalGlobalMem":2,
            "computeMajor":3,
            "computeMinor":4,
            "maxThreadsPerBlock":5,
            "maxThreadsPerMultiprocessor":6,
            "regsPerBlock":7,
            "warpSize":8,
            "sharedMemPerBlock":9,
            "multiProcessorCount":10,
            "regsPerMultiprocessor":11,
            "sharedMemPerBlockOptin":12,
            "numSms":13,
            "sharedMemPerMultiprocessor":13
        };
        assert_eq!(buf, expect);
        // println!(">>>{}<<<", _res);
        // println!("{}", buf);
    }
}
