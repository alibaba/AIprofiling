use super::{log, GenericErr};
use crate::{
    include_header, utils::parse_ts, GenericResult, HEADER_SECTION_SIZE, HEADER_SIZE, TILE_SIZE,
};
use jzon::JsonValue;
use pretty_hex::*;
use std::io::{BufRead as _, Cursor, Error, ErrorKind, Read, Result, Write};
include_header!("cupti_runtime_cbid");

#[derive(Debug)]
pub enum MsgType {
    EventCPU((usize, bool)),
    EventGPU(usize),
    Metadata,
    Props,
}

fn read_f32<T: AsRef<[u8]>>(buf: &mut Cursor<T>) -> Result<f32> {
    let mut value = [0u8; std::mem::size_of::<f32>()];
    buf.read_exact(&mut value)?;
    Ok(f32::from_le_bytes(value))
}

fn read_i32<T: AsRef<[u8]>>(buf: &mut Cursor<T>) -> Result<i32> {
    let mut value = [0u8; std::mem::size_of::<i32>()];
    buf.read_exact(&mut value)?;
    Ok(i32::from_le_bytes(value))
}

fn read_u32<T: AsRef<[u8]>>(buf: &mut Cursor<T>) -> Result<u32> {
    let mut value = [0u8; std::mem::size_of::<u32>()];
    buf.read_exact(&mut value)?;
    Ok(u32::from_le_bytes(value))
}

fn read_u64<T: AsRef<[u8]>>(buf: &mut Cursor<T>) -> Result<u64> {
    let mut value = [0u8; std::mem::size_of::<u64>()];
    buf.read_exact(&mut value)?;
    Ok(u64::from_le_bytes(value))
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

fn driver_flow_start(cbid: u32) -> bool {
    cbid == 307 || cbid == 652
}

pub fn parse_msg(input: &Vec<u8>, ts_off: u64) -> Result<(JsonValue, MsgType)> {
    let mut curr = Cursor::new(input);
    let read_kind = read_u32(&mut curr)?;
    match GpActivityType::from(read_kind) {
        GpActivityType::GpActivityKindRuntime => parse_runtime(&mut curr, ts_off),
        GpActivityType::GpActivityKindDriver => parse_driver(&mut curr, ts_off),
        GpActivityType::GpActivityKindConcurrentKernel => parse_kernel(&mut curr, ts_off),
        GpActivityType::GpActivityKindMemset => parse_memset(&mut curr, ts_off),
        GpActivityType::GpActivityKindMemcpy => parse_memcpy(&mut curr, ts_off),
        GpActivityType::GpActivityKindDevice => parse_device_prop(&mut curr, ts_off),
        GpActivityType::GpActivityKindMemcpy2 => parse_memcpy2(&mut curr, ts_off),
        GpActivityType::GpActivityKindSync => parse_synchronization(&mut curr, ts_off),
        GpActivityType::GpActivityKindCudaEvent => parse_cuda_event(&mut curr, ts_off),
        kind @ _ => Err(Error::new(
            ErrorKind::NotFound,
            format!(
                "unknown kind, read:{} (0x{:X}), kind: {:?}",
                read_kind, read_kind, kind
            ),
        )),
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

fn new_metadata_evnet(cat: &str) -> JsonValue {
    let mut buf = JsonValue::new_object();

    buf["ph"] = "M".into();
    buf["cat"] = JsonValue::from(cat);

    buf["args"] = JsonValue::new_object();
    _ = buf["args"].insert("External id", JsonValue::from(0));

    buf
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
fn parse_runtime<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("cuda_runtime");
    let cbid = read_u32(&mut input)?;
    if is_block_listed_runtime_cbid(cbid as u32) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "filter out listed runtime cbid",
        ));
    }
    let start = read_u64(&mut input)?;
    let duration = read_u64(&mut input)?;
    let process_id = read_u32(&mut input)?;
    let thread_id = read_u32(&mut input)?;
    let correlation_id = read_u32(&mut input)?;
    let mut name = Vec::new();
    input.read_until(b'\0', &mut name)?;
    let name = String::from_utf8(name.to_vec())
        .map_err(|e| Error::new(ErrorKind::UnexpectedEof, e))?
        .trim_matches('\0')
        .to_string();

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["name"] = JsonValue::from(name);
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);
    _ = buf["args"].insert("cbid", JsonValue::from(cbid));
    buf["pid"] = JsonValue::from(process_id);
    buf["tid"] = JsonValue::from(thread_id);
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    Ok((
        buf,
        MsgType::EventCPU((correlation_id as usize, flow_start(cbid as u32))),
    ))
}

fn parse_driver<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("cuda_driver");
    let cbid = read_u32(&mut input)?;
    //CUPTI_DRIVER_TRACE_CBID_cuLaunchKernel = 307
    // CUPTI_DRIVER_TRACE_CBID_cuLaunchKernelEx = 652
    if cbid != 307 && cbid != 652 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "filter out non cuLaunchKernel or cuLaunchKernelEx",
        ));
    }

    let start = read_u64(&mut input)?;
    let duration = read_u64(&mut input)?;
    let process_id = read_u32(&mut input)?;
    let thread_id = read_u32(&mut input)?;
    let correlation_id = read_u32(&mut input)?;
    let mut name = Vec::new();
    input.read_until(b'\0', &mut name)?;
    let name = String::from_utf8(name.to_vec())
        .map_err(|e| Error::new(ErrorKind::UnexpectedEof, e))?
        .trim_matches('\0')
        .to_string();

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["name"] = JsonValue::from(name);
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);
    _ = buf["args"].insert("cbid", JsonValue::from(cbid));
    buf["pid"] = JsonValue::from(process_id);
    buf["tid"] = JsonValue::from(thread_id);
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    Ok((
        buf,
        MsgType::EventCPU((correlation_id as usize, driver_flow_start(cbid as u32))),
    ))
}

// struct gp_proto_act_kernel {
// 	uint32_t kind;
// 	uint32_t correlationId;
// 	uint64_t start;
// 	uint64_t duration;
// 	int32_t gridX;
// 	int32_t gridY;
// 	int32_t gridZ;
// 	int32_t blockX;
// 	int32_t blockY;
// 	int32_t blockZ;
// 	uint32_t clusterX;
// 	uint32_t clusterY;
// 	uint32_t clusterZ;
// 	int32_t staticSharedMemory;
// 	int32_t dynamicSharedMemory;
// 	uint32_t deviceId;
// 	uint32_t contextId;
// 	uint32_t streamId;
// 	uint64_t graphNodeId;
// 	uint32_t graphId;
// 	uint32_t channelID;
// 	uint32_t channelType;
// 	char name[];
// };
fn parse_kernel<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("kernel");
    let correlation_id = read_u32(&mut input)?;
    let start = read_u64(&mut input)?;
    let duration = read_u64(&mut input)?;
    let grid_x = read_i32(&mut input)?;
    let grid_y = read_i32(&mut input)?;
    let grid_z = read_i32(&mut input)?;
    let block_x = read_i32(&mut input)?;
    let block_y = read_i32(&mut input)?;
    let block_z = read_i32(&mut input)?;
    let _cluster_x = read_u32(&mut input)?;
    let _cluster_y = read_u32(&mut input)?;
    let _cluster_z = read_u32(&mut input)?;
    let static_shared_memory = read_i32(&mut input)?;
    let dynamic_shared_memory = read_i32(&mut input)?;
    let device_id = read_u32(&mut input)?;
    let context_id = read_u32(&mut input)?;
    let stream_id = read_u32(&mut input)?;
    let _graph_node_id = read_u64(&mut input)?;
    let _graph_id = read_u32(&mut input)?;
    let _channel_id = read_u32(&mut input)?;
    let _channel_type = read_u32(&mut input)?;
    let registers_per_thread = read_u32(&mut input)?;
    let occupency = read_i32(&mut input)?;
    let block_per_sm = read_f32(&mut input)?;
    let wraps_per_sm = read_f32(&mut input)?;

    let mut name = Vec::new();
    input.read_until(b'\0', &mut name)?;
    let name = String::from_utf8(name.to_vec())
        .map_err(|e| Error::new(ErrorKind::UnexpectedEof, e))?
        .trim_matches('\0')
        .to_string();
    let name = cpp_demangle::Symbol::new(&name)
        .and_then(|sym| Ok(sym.to_string()))
        .unwrap_or(name);

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["name"] = JsonValue::from(name);
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));
    _ = buf["args"].insert(
        "grid",
        JsonValue::from(format!("[{}, {}, {}]", grid_x, grid_y, grid_z)),
    );
    _ = buf["args"].insert(
        "block",
        JsonValue::from(format!("[{}, {}, {}]", block_x, block_y, block_z)),
    );
    _ = buf["args"].insert(
        "shared memory",
        JsonValue::from(dynamic_shared_memory + static_shared_memory),
    );
    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));
    _ = buf["args"].insert("context", JsonValue::from(context_id));
    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));
    _ = buf["args"].insert(
        "registers per thread",
        JsonValue::from(registers_per_thread),
    );
    _ = buf["args"].insert("blocks per SM", JsonValue::from(block_per_sm));
    _ = buf["args"].insert("warps per SM", JsonValue::from(wraps_per_sm));
    _ = buf["args"].insert("est. achieved occupancy %", JsonValue::from(occupency));

    Ok((buf, MsgType::EventGPU(correlation_id as usize)))
}

// struct gp_proto_memset {
// 	uint32_t kind;
// 	uint32_t value;
// 	uint64_t start;
// 	uint64_t duration;
// 	uint64_t bytes;
// 	uint32_t correlationId;
// 	uint32_t deviceId;
// 	uint32_t contextId;
// 	uint32_t streamId;
// 	uint64_t graphNodeId;
// 	uint32_t graphId;
// 	uint32_t channelID;
// 	uint32_t channelType;
// };
fn parse_memset<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("gpu_memset");

    let _value = read_u32(&mut input)?;
    let start = read_u64(&mut input)?;
    let duration = read_u64(&mut input)?;
    let bytes = read_u64(&mut input)?;
    let correlation_id = read_u32(&mut input)?;
    let device_id = read_u32(&mut input)?;
    let context_id = read_u32(&mut input)?;
    let stream_id = read_u32(&mut input)?;
    let _graph_node_id = read_u64(&mut input)?;
    let _graph_id = read_u32(&mut input)?;
    let _channel_id = read_u32(&mut input)?;
    let _channel_type = read_u32(&mut input)?;

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    _ = buf["args"].insert("bytes", JsonValue::from(bytes));
    buf["name"] = JsonValue::from(format!("Memset (Device)"));

    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));

    _ = buf["args"].insert("context", JsonValue::from(context_id));

    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));

    Ok((buf, MsgType::EventGPU(correlation_id as usize)))
}

// struct gp_proto_memcpy {
// 	uint32_t kind;
// 	uint32_t copyKind;
// 	uint64_t start;
// 	uint64_t duration;
// 	uint64_t bytes;
// 	uint32_t srcKind;
// 	uint32_t dstKind;
// 	uint32_t correlationId;
// 	uint32_t deviceId;
// 	uint32_t contextId;
// 	uint32_t streamId;
// 	uint64_t graphNodeId;
// 	uint32_t graphId;
// 	uint32_t channelID;
// 	uint32_t channelType;
// };
fn parse_memcpy<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("gpu_memcpy");
    let copy_kind = read_u32(&mut input)?;
    let start = read_u64(&mut input)?;
    let duration = read_u64(&mut input)?;
    let bytes = read_u64(&mut input)?;
    let src_kind = read_u32(&mut input)?;
    let dst_kind = read_u32(&mut input)?;
    let correlation_id = read_u32(&mut input)?;
    let device_id = read_u32(&mut input)?;
    let context_id = read_u32(&mut input)?;
    let stream_id = read_u32(&mut input)?;
    let _graph_node_id = read_u64(&mut input)?;
    let _graph_id = read_u32(&mut input)?;
    let _channel_id = read_u32(&mut input)?;
    let _channel_type = read_u32(&mut input)?;

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    _ = buf["args"].insert("bytes", JsonValue::from(bytes));

    buf["name"] = JsonValue::from(format!(
        "Memcpy {} ({} -> {})",
        get_memcpy_kind(copy_kind),
        get_memory_kind(src_kind),
        get_memory_kind(dst_kind)
    ));

    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));
    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));
    _ = buf["args"].insert("context", JsonValue::from(context_id));
    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));

    Ok((buf, MsgType::EventGPU(correlation_id as usize)))
}
fn parse_memcpy2<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("gpu_memcpy2");
    let copy_kind = read_u32(&mut input)?;
    let start = read_u64(&mut input)?;
    let duration = read_u64(&mut input)?;
    let bytes = read_u64(&mut input)?;
    let src_kind = read_u32(&mut input)?;
    let dst_kind = read_u32(&mut input)?;
    let correlation_id = read_u32(&mut input)?;
    let device_id = read_u32(&mut input)?;
    let context_id = read_u32(&mut input)?;
    let stream_id = read_u32(&mut input)?;
    let _graph_node_id = read_u64(&mut input)?;
    let _graph_id = read_u32(&mut input)?;
    let _channel_id = read_u32(&mut input)?;
    let _channel_type = read_u32(&mut input)?;
    let src_device_id = read_u32(&mut input)?;
    let src_context_id = read_u32(&mut input)?;
    let dst_device_id = read_u32(&mut input)?;
    let dst_context_id = read_u32(&mut input)?;

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);

    _ = buf["args"].insert("bytes", JsonValue::from(bytes));

    buf["name"] = JsonValue::from(format!(
        "Memcpy {} ({} -> {})",
        get_memcpy_kind(copy_kind),
        get_memory_kind(src_kind),
        get_memory_kind(dst_kind)
    ));

    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));
    buf["pid"] = JsonValue::from(device_id);
    _ = buf["args"].insert("device", JsonValue::from(device_id));
    _ = buf["args"].insert("context", JsonValue::from(context_id));
    buf["tid"] = JsonValue::from(stream_id);
    _ = buf["args"].insert("stream", JsonValue::from(stream_id));

    _ = buf["args"].insert("src_device", JsonValue::from(src_device_id));
    _ = buf["args"].insert("src_context", JsonValue::from(src_context_id));
    _ = buf["args"].insert("dst_device", JsonValue::from(dst_device_id));
    _ = buf["args"].insert("dst_context", JsonValue::from(dst_context_id));

    Ok((buf, MsgType::EventGPU(correlation_id as usize)))
}

// struct gp_proto_sync {
// 	uint32_t kind;
// 	uint32_t type;
// 	uint64_t start;
// 	uint64_t end;
// 	uint32_t correlationId;
// 	uint32_t contextId;
// 	uint32_t streamId;
// 	uint32_t cudaEventId;
// };
fn parse_synchronization<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_trace_evnet("synchronization");
    let sync_type = read_u32(&mut input)?;
    let start = read_u64(&mut input)?;
    let end = read_u64(&mut input)?;
    let duration = end - start;
    let correlation_id = read_u32(&mut input)?;
    let context_id = read_u32(&mut input)?;
    let stream_id = read_u32(&mut input)?;
    let cuda_event_id = read_u32(&mut input)?;

    let ts = parse_ts(start, ts_off)?;
    let dur = duration as f64 / 1000.0;

    buf["name"] = JsonValue::from(format!(
        "synchronization({})",
        get_synchronization_type(sync_type)
    ));
    buf["ts"] = JsonValue::from(ts);
    buf["dur"] = JsonValue::from(dur);
    _ = buf["args"].insert("context_id", JsonValue::from(context_id));
    _ = buf["args"].insert("stream_id", JsonValue::from(stream_id));
    _ = buf["args"].insert("cuda_event_id", JsonValue::from(cuda_event_id));
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    Ok((buf, MsgType::EventGPU(correlation_id as usize)))
}

fn get_synchronization_type(sync_type: u32) -> String {
    match sync_type {
        0 => "UNKNOWN".to_string(),
        1 => "EVENT_SYNCHRONIZE".to_string(),
        2 => "STREAM_WAIT_EVENT".to_string(),
        3 => "STREAM_SYNCHRONIZE".to_string(),
        4 => "CONTEXT_SYNCHRONIZE".to_string(),
        _ => "<unknown>".to_string(),
    }
}

// /* for CUPTI_ACTIVITY_KIND_CUDA_EVENT */
// struct gp_proto_cuda_event {
// 	uint32_t kind;
// 	uint32_t correlationId;
// 	uint32_t contextId;
// 	uint32_t streamId;
// 	uint32_t eventId;
// };
fn parse_cuda_event<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    _ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = new_metadata_evnet("cuda_event");
    let correlation_id = read_u32(&mut input)?;
    let context_id = read_u32(&mut input)?;
    let stream_id = read_u32(&mut input)?;
    let cuda_event_id = read_u32(&mut input)?;

    buf["name"] = JsonValue::from("cuda_event");
    _ = buf["args"].insert("context_id", JsonValue::from(context_id));
    _ = buf["args"].insert("stream_id", JsonValue::from(stream_id));
    _ = buf["args"].insert("cuda_event_id", JsonValue::from(cuda_event_id));
    _ = buf["args"].insert("correlation", JsonValue::from(correlation_id));

    Ok((buf, MsgType::Metadata))
}

// static const char *
// GetMemcpyKindString(
//     CUpti_ActivityMemcpyKind memcpyKind)
// {
//     switch (memcpyKind)
//     {
//         case CUPTI_ACTIVITY_MEMCPY_KIND_UNKNOWN:
//             return "UNKNOWN";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_HTOD:
//             return "HtoD";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_DTOH:
//             return "DtoH";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_HTOA:
//             return "HtoA";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_ATOH:
//             return "AtoH";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_ATOA:
//             return "AtoA";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_ATOD:
//             return "AtoD";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_DTOA:
//             return "DtoA";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_DTOD:
//             return "DtoD";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_HTOH:
//             return "HtoH";
//         case CUPTI_ACTIVITY_MEMCPY_KIND_PTOP:
//             return "PtoP";
//         default:
//             return "<unknown>";
//     }
// }
fn get_memcpy_kind(kind: u32) -> String {
    match kind {
        0 => "UNKNOWN".to_string(),
        1 => "HtoD".to_string(),
        2 => "DtoH".to_string(),
        3 => "HtoA".to_string(),
        4 => "AtoH".to_string(),
        5 => "AtoA".to_string(),
        6 => "AtoD".to_string(),
        7 => "DtoA".to_string(),
        8 => "DtoD".to_string(),
        9 => "HtoH".to_string(),
        10 => "PtoP".to_string(),
        _ => "<unknown>".to_string(),
    }
}

// static const char *
// GetMemoryKindString(
//     CUpti_ActivityMemoryKind memoryKind)
// {
//     switch (memoryKind)
//     {
//         case CUPTI_ACTIVITY_MEMORY_KIND_UNKNOWN:
//             return "UNKNOWN";
//         case CUPTI_ACTIVITY_MEMORY_KIND_PAGEABLE:
//             return "PAGEABLE";
//         case CUPTI_ACTIVITY_MEMORY_KIND_PINNED:
//             return "PINNED";
//         case CUPTI_ACTIVITY_MEMORY_KIND_DEVICE:
//             return "DEVICE";
//         case CUPTI_ACTIVITY_MEMORY_KIND_ARRAY:
//             return "ARRAY";
//         case CUPTI_ACTIVITY_MEMORY_KIND_MANAGED:
//             return "MANAGED";
//         case CUPTI_ACTIVITY_MEMORY_KIND_DEVICE_STATIC:
//             return "DEVICE_STATIC";
//         case CUPTI_ACTIVITY_MEMORY_KIND_MANAGED_STATIC:
//             return "MANAGED_STATIC";
//         default:
//             return "<unknown>";
//     }
// }
fn get_memory_kind(kind: u32) -> String {
    match kind {
        0 => "UNKNOWN".to_string(),
        1 => "PAGEABLE".to_string(),
        2 => "PINNED".to_string(),
        3 => "DEVICE".to_string(),
        4 => "ARRAY".to_string(),
        5 => "MANAGED".to_string(),
        6 => "DEVICE_STATIC".to_string(),
        7 => "MANAGED_STATIC".to_string(),
        _ => "<unknown>".to_string(),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum GpType {
    #[default]
    GpInvalid,
    GpExit,
    GpActivity,
    GpGetDevProp,
}

impl From<u32> for GpType {
    fn from(value: u32) -> Self {
        match value {
            1 => GpType::GpActivity,
            2 => GpType::GpGetDevProp,
            0xdeadbeef => GpType::GpExit,
            _ => GpType::GpInvalid,
        }
    }
}

#[derive(Debug)]
pub enum GpActivityType {
    GpActivityKindRuntime,
    GpActivityKindDriver,
    GpActivityKindConcurrentKernel,
    GpActivityKindMemset,
    GpActivityKindMemcpy,
    GpActivityKindDevice,
    GpActivityKindMemcpy2,   //A peer to peer memory copy
    GpActivityKindCudaEvent, // Information about a CUDA event.
    GpActivityKindSync,      // Records for synchronization management.
    GpActivityKindUnkown,
}

impl From<u32> for GpActivityType {
    fn from(value: u32) -> Self {
        match value {
            1 => GpActivityType::GpActivityKindMemcpy,
            2 => GpActivityType::GpActivityKindMemset,
            4 => GpActivityType::GpActivityKindDriver,
            5 => GpActivityType::GpActivityKindRuntime,
            8 => GpActivityType::GpActivityKindDevice,
            10 => GpActivityType::GpActivityKindConcurrentKernel,
            22 => GpActivityType::GpActivityKindMemcpy2,
            36 => GpActivityType::GpActivityKindCudaEvent,
            38 => GpActivityType::GpActivityKindSync,
            _ => GpActivityType::GpActivityKindUnkown,
        }
    }
}

// struct gp_proto_dev_prop {
//  uin32_t kind;
//  int id;
// 	char name[256];
// 	uint64_t totalGlobalMem;
// 	uint64_t sharedMemPerBlock;
// 	int major;
// 	int minor;
// 	int maxThreadsPerBlock;
// 	int maxThreadsPerMultiProcessor;
// 	int regsPerBlock;
// 	int warpSize;
// 	int multiProcessorCount;
// 	int regsPerMultiprocessor;
// 	uint64_t sharedMemPerBlockOptin;
// 	uint64_t sharedMemPerMultiprocessor;
// 	int id;
// };

pub fn parse_device_prop<T: AsRef<[u8]>>(
    mut input: &mut Cursor<T>,
    _ts_off: u64,
) -> Result<(JsonValue, MsgType)> {
    let mut buf = JsonValue::new_object();
    let id = read_i32(&mut input)?;
    let mut name = [0u8; 256];
    input.read_exact(&mut name)?;

    let total_global_mem = read_u64(&mut input)?;
    let shared_mem_per_block = read_u64(&mut input)?;
    let major = read_i32(&mut input)?;
    let minor = read_i32(&mut input)?;
    let max_threads_per_block = read_i32(&mut input)?;
    let max_threads_per_multi_processor = read_i32(&mut input)?;
    let regs_per_block = read_i32(&mut input)?;
    let warp_size = read_i32(&mut input)?;
    let multi_processor_count = read_i32(&mut input)?;
    let regs_per_multi_processor = read_i32(&mut input)?;
    let shared_mem_per_block_option = read_u64(&mut input)?;
    let shared_mem_per_multiprocessor = read_u64(&mut input)?;

    let name = String::from_utf8(name.to_vec())
        .map_err(|e| Error::new(ErrorKind::UnexpectedEof, e))?
        .trim_matches('\0')
        .to_string();
    buf["name"] = JsonValue::from(name);
    buf["totalGlobalMem"] = JsonValue::from(total_global_mem);
    buf["computeMajor"] = JsonValue::from(major);
    buf["computeMinor"] = JsonValue::from(minor);
    buf["maxThreadsPerBlock"] = JsonValue::from(max_threads_per_block);
    buf["maxThreadsPerMultiprocessor"] = JsonValue::from(max_threads_per_multi_processor);
    buf["regsPerBlock"] = JsonValue::from(regs_per_block);
    buf["warpSize"] = JsonValue::from(warp_size);
    buf["sharedMemPerBlock"] = JsonValue::from(shared_mem_per_block);
    buf["multiProcessorCount"] = JsonValue::from(multi_processor_count);
    buf["regsPerMultiprocessor"] = JsonValue::from(regs_per_multi_processor);
    buf["sharedMemPerBlockOptin"] = JsonValue::from(shared_mem_per_block_option);
    buf["numSms"] = JsonValue::from(shared_mem_per_multiprocessor);
    buf["sharedMemPerMultiprocessor"] = JsonValue::from(shared_mem_per_multiprocessor);
    buf["id"] = JsonValue::from(id);

    Ok((buf, MsgType::Props))
}

#[derive(Debug, Clone, Default)]
pub enum BinParserState {
    #[default]
    Reload,
    Readable((GpType, Vec<u8>)),
}

#[derive(Debug, Clone)]
pub struct BinParser {
    cursor: Cursor<[u8; 2 * TILE_SIZE]>,
    tail: usize,
    occupancy: usize,
    pub count: usize,
    state: BinParserState,
    next_state: BinParserState,
}

impl Default for BinParser {
    fn default() -> Self {
        Self {
            tail: 0,
            cursor: Cursor::new([0; 2 * TILE_SIZE]),
            occupancy: 0,
            count: 0,
            state: BinParserState::default(),
            next_state: BinParserState::default(),
        }
    }
}

impl BinParser {
    pub fn next(&mut self) -> Result<BinParserState> {
        match self.next_state {
            // readable when buffer is not empty, and a cursor will run thought buffer,
            // return every line and set status to readable until cursor reach buffer end.
            BinParserState::Readable(_) => {
                let last_pos = self.cursor.position() as usize;

                let need = HEADER_SIZE;
                if need > self.occupancy {
                    self.next_state = BinParserState::Reload;
                    self.state = BinParserState::Reload;
                    return Ok(self.state.clone());
                }

                let mut header = [0u8; HEADER_SIZE];

                let circle_break = need.saturating_sub((last_pos + need) % (2 * TILE_SIZE));

                if circle_break > 0 {
                    let (mut tail, mut head) = header.split_at_mut(circle_break);
                    self.cursor.read(&mut tail)?;
                    self.cursor.set_position(0);
                    self.cursor.read(&mut head)?;
                } else {
                    self.cursor.read(&mut header)?;
                }

                let (gp_type, len) = header.split_at(HEADER_SECTION_SIZE);
                let occupancy = self.occupancy - HEADER_SIZE;

                let need = u32::from_le_bytes(
                    len.try_into()
                        .map_err(|e| Error::new(ErrorKind::UnexpectedEof, e))?,
                ) as usize;
                let gt = GpType::from(u32::from_le_bytes(
                    gp_type
                        .try_into()
                        .map_err(|e| Error::new(ErrorKind::UnexpectedEof, e))?,
                ));
                log::trace!(
                    "read header: gp_type: {:?}, len: {} 0x{:X}, occupancy: {} 0x{:X}",
                    gt,
                    need,
                    need,
                    occupancy,
                    occupancy
                );
                match gt {
                    GpType::GpInvalid => {
                        log::error!("invalid gp_type: {:?}", gt);
                        log::trace!("read header: \n{:?}", header.hex_dump());
                        return Err(Error::new(ErrorKind::InvalidData, "invalid gp_type"));
                    }
                    GpType::GpExit => {
                        self.occupancy = occupancy;
                        if occupancy != 0 {
                            log::error!("occupancy not zero: {}", occupancy);
                        }
                        self.next_state = BinParserState::Reload;
                        self.state = BinParserState::Reload;
                        return Ok(BinParserState::Readable((gt, Vec::new())));
                    }
                    _ => {}
                }

                if need > occupancy {
                    self.cursor.set_position(last_pos as u64);
                    self.next_state = BinParserState::Reload;
                    self.state = BinParserState::Reload;
                    return Ok(self.state.clone());
                }

                let mut contents = vec![0u8; need];
                let circle_break =
                    need.saturating_sub((last_pos + HEADER_SIZE + need) % (2 * TILE_SIZE));

                if circle_break > 0 {
                    let (mut tail, mut head) = contents.split_at_mut(circle_break);
                    self.cursor.read(&mut tail)?;
                    self.cursor.set_position(0);
                    self.cursor.read(&mut head)?;
                } else {
                    self.cursor.read(&mut contents)?;
                }

                self.next_state = BinParserState::Readable((GpType::default(), Vec::new()));
                self.state = BinParserState::Readable((gt, contents));
                self.occupancy = occupancy - need;
                self.count += 1;
            }

            BinParserState::Reload => {
                self.state = BinParserState::Reload;
                self.next_state = BinParserState::Reload;
            }
        }
        Ok(self.state.clone())
    }

    pub fn reload(&mut self, input: [u8; TILE_SIZE], size: usize) -> GenericResult {
        if size > TILE_SIZE || size == 0 {
            return Err(GenericErr::from("invalid size"));
        }
        // current pos AKA available front
        let current_pos = self.cursor.position() as usize;

        let circle_break = size.saturating_sub((self.tail + size) % (2 * TILE_SIZE));

        log::trace!(
            "reload {}, current_pos: {}, tail_pos: {}, occupancy: {}",
            size,
            current_pos,
            self.tail,
            self.occupancy,
        );

        // trim the input data
        let (buf, _) = input.split_at(size);
        self.cursor.set_position(self.tail as u64);

        if circle_break > 0 {
            let (end_part, front_part) = buf.split_at(circle_break);
            self.cursor.write(end_part)?;
            self.cursor.set_position(0);
            self.cursor.write(front_part)?;
            self.tail = size - circle_break;
        } else {
            self.cursor.write(buf.as_ref())?;
            self.tail += size;
        }
        self.occupancy += size;
        self.cursor.set_position(current_pos as u64);

        self.next_state = BinParserState::Readable((GpType::default(), Vec::new()));
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "protocal_gp")]
mod tests {
    use super::*;
    use rand::Rng;
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
        let mut rng = rand::rng();
        let fd = std::fs::File::open("../testcases/bin-data.json").unwrap();
        let file_len = fd.metadata().unwrap().len() as usize;
        let mut buf = std::io::BufReader::new(fd);
        let mut tile = [0u8; TILE_SIZE];
        let mut count: usize = 0;
        let mut flag = false;
        let mut parser = BinParser::default();
        let mut content_len: usize = 0;

        loop {
            let read_len = rng.random::<u32>() as usize % TILE_SIZE;
            match buf.read(&mut tile[..read_len]) {
                Ok(0) => {
                    if flag {
                        log::info!("EOF, Read: {}, content len: {}", count, content_len);
                        log::info!("count: {}", parser.count);
                        break;
                    }
                }
                Ok(n) => {
                    count += n;
                    if !flag {
                        flag = true;
                    }
                    parser.reload(tile, n).unwrap();
                    while let BinParserState::Readable((ty, contents)) = parser.next().unwrap() {
                        if ty == GpType::GpExit {
                            break;
                        }
                        content_len += contents.len();
                        let _a = parse_msg(&contents, 0);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    continue;
                }
                Err(err) => {
                    log::error!("read error: {}", err);
                    break;
                }
            }
        }
        assert_eq!(count, file_len);
        log::info!(
            "information density: {:.2}%",
            content_len as f64 / file_len as f64 * 100.0
        );
    }
}
