use chrono::{DateTime, Local};
use prost::Message;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::ffi::CStr;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::vec;

use crate::logging::*;
use crate::profile::*;
use flate2::Compression;
use flate2::write::GzEncoder;

const MAGIC: &[u8; 12] = b"PYKI-PROFILE";
const STRING_ID: u8 = 1;
const STACK_TRACE_ID: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AllocatorKind {
    SimpleAllocator = 1,
    SimpleDeallocator = 2,
    RangedAllocator = 3,
    RangedDeallocator = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Allocator {
    PymallocFree = 1,
    PymallocMalloc = 2,
    PymallocCalloc = 3,
    PymallocRealloc = 4,
    Free = 5,
    Malloc = 6,
    Realloc = 7,

    Calloc = 8,
    PosixMemalign = 9,
    AlignedAlloc = 10,
    Memalign = 11,
    Valloc = 12,
    Pvalloc = 13,
    Mmap = 14,
    Munmap = 15,
}

impl Allocator {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::PymallocFree),
            2 => Some(Self::PymallocMalloc),
            3 => Some(Self::PymallocCalloc),
            4 => Some(Self::PymallocRealloc),
            5 => Some(Self::Free),
            6 => Some(Self::Malloc),
            7 => Some(Self::Realloc),
            8 => Some(Self::Calloc),
            9 => Some(Self::PosixMemalign),
            10 => Some(Self::AlignedAlloc),
            11 => Some(Self::Memalign),
            12 => Some(Self::Valloc),
            13 => Some(Self::Pvalloc),
            14 => Some(Self::Mmap),
            15 => Some(Self::Munmap),
            _ => None,
        }
    }
}

pub fn allocator_kind(allocator: Allocator) -> AllocatorKind {
    match allocator {
        Allocator::Calloc
        | Allocator::Malloc
        | Allocator::Memalign
        | Allocator::PosixMemalign
        | Allocator::AlignedAlloc
        | Allocator::Pvalloc
        | Allocator::Realloc
        | Allocator::Valloc
        | Allocator::PymallocMalloc
        | Allocator::PymallocCalloc
        | Allocator::PymallocRealloc => AllocatorKind::SimpleAllocator,

        Allocator::Free | Allocator::PymallocFree => AllocatorKind::SimpleDeallocator,

        Allocator::Mmap => AllocatorKind::RangedAllocator,
        Allocator::Munmap => AllocatorKind::RangedDeallocator,
    }
}

pub fn is_deallocator(allocator: Allocator) -> bool {
    match allocator_kind(allocator) {
        AllocatorKind::SimpleAllocator | AllocatorKind::RangedAllocator => false,
        AllocatorKind::SimpleDeallocator | AllocatorKind::RangedDeallocator => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub begin: u64,
    pub end: u64,
}

impl Interval {
    pub fn new(begin: u64, end: u64) -> Self {
        Self { begin, end }
    }

    /// Returns the overlapping interval, if any.
    pub fn intersection(&self, other: &Interval) -> Option<Interval> {
        let b = self.begin.max(other.begin);
        let e = self.end.min(other.end);
        if b < e {
            Some(Interval::new(b, e))
        } else {
            None
        }
    }

    /// True if `self` is an intersection that touches the left side of `other`
    /// (i.e., it starts at `other.begin` but doesn't cover all of `other`).
    pub fn left_intersects(&self, other: &Interval) -> bool {
        self.begin == other.begin && self.end < other.end
    }

    /// True if `self` is an intersection that touches the right side of `other`
    /// (i.e., it ends at `other.end` but doesn't cover all of `other`).
    pub fn right_intersects(&self, other: &Interval) -> bool {
        self.end == other.end && self.begin > other.begin
    }

    pub fn size(&self) -> u64 {
        self.end.saturating_sub(self.begin)
    }
}

#[derive(Debug, Clone)]
pub struct IntervalTree<T> {
    intervals: Vec<(Interval, T)>,
}

impl<T> IntervalTree<T> {
    pub fn new() -> Self {
        Self {
            intervals: Vec::new(),
        }
    }

    pub fn add_interval(&mut self, start: u64, size: u64, element: T) {
        if size == 0 {
            return;
        }
        self.intervals
            .push((Interval::new(start, start + size), element));
    }

    pub fn size(&self) -> u64 {
        self.intervals.iter().map(|(i, _)| i.size()).sum()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, (Interval, T)> {
        self.intervals.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, (Interval, T)> {
        self.intervals.iter_mut()
    }
}

impl<T: Clone> IntervalTree<T> {
    pub fn remove_interval(&mut self, start: u64, size: u64) -> () {
        if size == 0 {
            return;
        }

        let removed_interval = Interval::new(start, start + size);

        let mut new_intervals: Vec<(Interval, T)> = Vec::with_capacity(self.intervals.len() + 1);

        for (interval, value) in self.intervals.drain(..) {
            let maybe_intersection = interval.intersection(&removed_interval);

            let Some(intersection) = maybe_intersection else {
                // Keep this interval entirely.
                new_intervals.push((interval, value));
                continue;
            };

            if intersection == interval {
                // Keep none of this interval (removed interval contains it).
                continue;
            } else if intersection.left_intersects(&interval) {
                // Keep the end of this interval (removed overlaps the start).
                new_intervals.push((Interval::new(intersection.end, interval.end), value.clone()));
            } else if intersection.right_intersects(&interval) {
                // Keep the start of this interval (removed overlaps the end).
                new_intervals.push((Interval::new(interval.begin, intersection.begin), value.clone()));
            } else {
                // Split this interval in two (removed overlaps the middle).
                new_intervals.push((Interval::new(interval.begin, intersection.begin), value.clone()));
                new_intervals.push((Interval::new(intersection.end, interval.end), value.clone()));
            }
        }

        self.intervals = new_intervals;
    }
}

fn read_uint<'a>(
    reader: &mut BufReader<File>,
    size: usize,
    error_msg: &'a str,
) -> Result<u64, &'a str> {
    match size {
        4 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf).map_err(|_| error_msg)?;
            Ok(u32::from_le_bytes(buf) as u64)
        }
        8 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf).map_err(|_| error_msg)?;
            Ok(u64::from_le_bytes(buf))
        }
        _ => Err("Should not reach here."),
    }
}

fn read_var_uint32<'a>(
    reader: &mut BufReader<File>,
    nread: &mut u64,
    error_msg: &'a str,
) -> Result<u32, &'a str> {
    let byte_buffer = &mut [0u8; 1];
    let mut value = 0u32;
    let mut count = 0u32;
    loop {
        reader.read_exact(byte_buffer).map_err(|_| error_msg)?;
        let byte = byte_buffer[0];
        if count < 4 {
            value |= ((byte & 0x7F) as u32) << (7 * count);
        } else {
            value |= ((byte & 0xFF) as u32) << (7 * count);
        }
        count += 1;
        if count == 5 || byte & 0x80 == 0 {
            break;
        }
    }
    *nread += count as u64;
    Ok(value)
}

fn read_var_uint64<'a>(
    reader: &mut BufReader<File>,
    nread: &mut u64,
    error_msg: &'a str,
) -> Result<u64, &'a str> {
    let byte_buffer = &mut [0u8; 1];
    let mut value = 0u64;
    let mut count = 0u32;
    loop {
        reader.read_exact(byte_buffer).map_err(|_| error_msg)?;
        let byte = byte_buffer[0];
        if count < 8 {
            value |= ((byte & 0x7F) as u64) << (7 * count);
        } else {
            value |= ((byte & 0xFF) as u64) << (7 * count);
        }
        count += 1;
        if count == 9 || byte & 0x80 == 0 {
            break;
        }
    }
    *nread += count as u64;
    Ok(value)
}

fn read_uint8<'a>(
    reader: &mut BufReader<File>,
    nread: &mut u64,
    error_msg: &'a str,
) -> Result<u8, &'a str> {
    let byte_buffer = &mut [0u8; 1];
    reader.read_exact(byte_buffer).map_err(|_| error_msg)?;
    *nread += 1;
    let allocator = byte_buffer[0];
    Ok(allocator)
}

fn millis_to_time_string(milliseconds: u64) -> String {
    let dt = DateTime::from_timestamp_millis(milliseconds as i64);

    match dt {
        Some(dt) => dt
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S.%3f")
            .to_string(),

        None => "Invalid timestamp.".to_string(),
    }
}

struct Parser<'a> {
    input: &'a str,
    header: Header,
    strings: HashMap<u32, String>,

    stack_traces: HashMap<u32, StackTrace>,
    cpu_events: Vec<CPUEvent>,
    allocation_events: Vec<AllocationEvent>,
    malloc_events: Vec<MallocEvent>,

    string_id_map: HashMap<u32, u32>,
    string_table: Vec<String>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        let mut p = Self {
            input,
            header: Header::default(),
            strings: HashMap::new(),
            stack_traces: HashMap::new(),
            cpu_events: Vec::new(),
            allocation_events: Vec::new(),
            malloc_events: Vec::new(),

            string_id_map: HashMap::new(),
            string_table: Vec::new(),
        };

        p.string_table.push("".to_string()); // 0
        p.string_table.push("cpu".to_string()); // 1
        p.string_table.push("milliseconds".to_string()); // 2
        p.string_table.push("alloc_space".to_string()); // 3
        p.string_table.push("bytes".to_string()); // 4
        p.string_table.push("samples".to_string()); // 5
        p.string_table.push("count".to_string()); // 6

        p
    }

    fn parse(&mut self) -> Result<(), Cow<'static, str>> {
        let file =
            File::open(self.input).map_err(|_| format!("Failed to open file: {}.", self.input))?;
        let length = file
            .metadata()
            .map(|m| m.len())
            .map_err(|_| "Failed to get file metadata.")?;

        let reader = &mut BufReader::new(file);

        // 1. Parse Header
        self.header.parse(reader)?;

        let mut nread = 48u64;
        let cpo = self.header.constant_pool_offset;

        // 2. Parse Events
        loop {
            if nread == cpo {
                break;
            } else if nread > cpo {
                return Err("Read past constant pool offset.".into());
            }

            let event_type = &mut [0u8; 1];
            reader
                .read_exact(event_type)
                .map_err(|_| "Failed to read event type.")?;
            nread += 1;
            let event_type = event_type[0];

            match event_type {
                CPUEvent::ID => {
                    self.cpu_events.push(CPUEvent::parse(reader, &mut nread)?);
                }
                AllocationEvent::ID => {
                    self.allocation_events
                        .push(AllocationEvent::parse(reader, &mut nread)?);
                }
                MallocEvent::ID => {
                    self.malloc_events.push(MallocEvent::parse(reader, &mut nread)?);
                }
                _ => {
                    return Err(format!(
                        "Unknown event type: {} at offset {}.",
                        event_type,
                        nread - 1
                    )
                    .into());
                }
            }
        }

        // 3. Parse Constant Pool
        loop {
            let constant_pool_type = &mut [0u8; 1];
            reader
                .read_exact(constant_pool_type)
                .map_err(|_| "Failed to read constant pool type.")?;
            nread += 1;
            let constant_pool_type = constant_pool_type[0];

            if constant_pool_type == STRING_ID {
                let length =
                    read_var_uint32(reader, &mut nread, "Failed to read string constant length.")?;
                for _ in 0..length {
                    let string_id =
                        read_var_uint32(reader, &mut nread, "Failed to read string id.")?;
                    let string_kind = &mut [0u8; 1];
                    reader
                        .read_exact(string_kind)
                        .map_err(|_| "Failed to read string kind.")?;
                    nread += 1;

                    let string_len =
                        read_var_uint32(reader, &mut nread, "Failed to read string length.")?;
                    let mut string_buf = vec![0u8; string_len as usize];
                    reader
                        .read_exact(&mut string_buf)
                        .map_err(|_| "Failed to read string data.")?;
                    nread += string_len as u64;

                    let string = match string_kind[0] {
                        0 => String::from_utf8(string_buf).map_err(|_| "Failed to read string.")?,
                        1 => string_buf.iter().map(|&b| b as char).collect(),
                        2 => {
                            #[allow(clippy::cast_ptr_alignment)]
                            let chars = unsafe {
                                std::slice::from_raw_parts(
                                    string_buf.as_ptr() as *const u16,
                                    string_buf.len() / 2,
                                )
                            };
                            String::from_utf16(chars).map_err(|_| "Failed to read string.")?
                        }
                        3 => {
                            #[allow(clippy::cast_ptr_alignment)]
                            let chars = unsafe {
                                std::slice::from_raw_parts(
                                    string_buf.as_ptr() as *const char,
                                    string_buf.len() / 4,
                                )
                            };
                            chars.iter().collect()
                        }
                        _ => {
                            return Err("Failed to read string.".into());
                        }
                    };

                    self.string_id_map
                        .insert(string_id, self.string_table.len() as u32);
                    self.string_table.push(string.clone());

                    self.strings.insert(string_id, string);
                }
            } else if constant_pool_type == STACK_TRACE_ID {
                let length = read_var_uint32(
                    reader,
                    &mut nread,
                    "Failed to read stack trace constant length.",
                )?;

                for _ in 0..length {
                    let stack_trace_id =
                        read_var_uint32(reader, &mut nread, "Failed to read stack trace id.")?;
                    let frame_count =
                        read_var_uint32(reader, &mut nread, "Failed to read frame count.")?;
                    let mut frames = Vec::with_capacity(frame_count as usize);
                    for _ in 0..frame_count {
                        let filename = read_var_uint32(
                            reader,
                            &mut nread,
                            "Failed to read frame filename id.",
                        )?;
                        let name =
                            read_var_uint32(reader, &mut nread, "Failed to read frame name id.")?;
                        let lineno =
                            read_var_uint32(reader, &mut nread, "Failed to read frame lineno.")?;
                        frames.push(Frame {
                            filename,
                            name,
                            lineno,
                        });
                    }
                    self.stack_traces
                        .insert(stack_trace_id, StackTrace { frames });
                }
            } else {
                return Err(format!(
                    "Unknown constant pool type: {} at offset {}.",
                    constant_pool_type, nread
                )
                .into());
            }
            if nread == length {
                break;
            } else if nread > length {
                return Err("Read past end of file.".into());
            }
        }

        for stack_trace in self.stack_traces.values_mut() {
            for frame in &mut stack_trace.frames {
                frame.filename = self
                    .string_id_map
                    .get(&frame.filename)
                    .map_or(0, |v| *v as u32);
                frame.name = self.string_id_map.get(&frame.name).map_or(0, |v| *v as u32);
            }
        }

        Ok(())
    }

    fn print_summary(&self) {
        println!("{}", self.header);
        println!("Total Strings: {}", self.strings.len());
        println!("Total Stack Traces: {}", self.stack_traces.len());
        println!("Total CPU Events: {}", self.cpu_events.len());
        println!("Total Allocation Events: {}", self.allocation_events.len());
        println!("Total Malloc Events: {}", self.malloc_events.len());
    }

    fn print_events(&self) {
        for event in &self.cpu_events {
            println!("CPU Event:");
            println!("  Thread ID: {}", event.thread_id);
            println!("  Stack Trace:");
            self.print_stack_trace(event.stack_trace_id);
            println!()
        }

        for event in &self.allocation_events {
            println!("Allocation Event:");
            println!("  Thread ID: {}", event.thread_id);
            println!("  Size: {} bytes", event.size);
            println!("  Stack Trace:");
            self.print_stack_trace(event.stack_trace_id);
            println!()
        }

        for event in &self.malloc_events {
            println!("Malloc Event:");
            println!("  Thread ID: {}", event.thread_id);
            println!("  Size: {} bytes", event.size);
            println!("  Address: {}", event.address);
            println!("  Stack Trace:");
            self.print_stack_trace(event.stack_trace_id);
            println!()
        }
    }

    fn print_stack_trace(&self, id: u32) {
        if let Some(stack_trace) = self.stack_traces.get(&id) {
            for frame in &stack_trace.frames {
                println!(
                    "    at {} ({}:{})",
                    self.get_string(frame.filename),
                    self.get_string(frame.name),
                    frame.lineno
                );
            }
        } else {
            println!("  <unknown stack trace id: {}>", id);
        }
    }

    fn get_string(&self, id: u32) -> &str {
        self.string_table
            .get(id as usize)
            .map_or("<unknown>", |s| s.as_str())
    }
}

#[derive(Debug, Default)]
struct Header {
    magic: [u8; 12],
    version: u64,
    size: u64,
    start_time: u64,
    end_time: u64,
    constant_pool_offset: u64,
}

impl Header {
    fn parse(&mut self, reader: &mut BufReader<File>) -> Result<(), &'static str> {
        reader
            .read_exact(&mut self.magic)
            .map_err(|_| "Failed to read magic.")?;

        if &self.magic != MAGIC {
            return Err("Invalid magic.");
        }

        self.version = read_uint(reader, 4, "Failed to read version.")?;
        self.size = read_uint(reader, 8, "Failed to read size.")?;
        self.start_time = read_uint(reader, 8, "Failed to read start_time.")?;
        self.end_time = read_uint(reader, 8, "Failed to read end_time.")?;
        self.constant_pool_offset = read_uint(reader, 8, "Failed to read constant_pool_offset.")?;

        return Ok(());
    }
}

impl Display for Header {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Profile Header:\n")?;
        write!(
            f,
            "  {: <21} {}\n",
            "Magic:",
            String::from_utf8_lossy(&self.magic)
        )?;
        write!(f, "  {: <21} {}\n", "Version:", self.version)?;
        write!(f, "  {: <21} {}\n", "Size:", self.size)?; // Rust's fmt does not have a simple ':,', so we omit it
        write!(
            f,
            "  {: <21} {}\n",
            "Start Time:",
            millis_to_time_string(self.start_time)
        )?;
        write!(
            f,
            "  {: <21} {}\n",
            "End Time:",
            millis_to_time_string(self.end_time)
        )?;
        write!(
            f,
            "  {: <21} {}\n",
            "Constant Pool Offset:", self.constant_pool_offset
        )
    }
}

struct Frame {
    filename: u32,
    name: u32,
    lineno: u32,
}

struct StackTrace {
    frames: Vec<Frame>,
}

trait Event
where
    Self: Sized,
{
    const ID: u8;

    fn parse(reader: &mut BufReader<File>, nmread: &mut u64) -> Result<Self, &'static str>;
}

struct CPUEvent {
    thread_id: u32,
    stack_trace_id: u32,
}

impl Event for CPUEvent {
    const ID: u8 = 32;

    fn parse(reader: &mut BufReader<File>, nread: &mut u64) -> Result<Self, &'static str> {
        let thread_id = read_var_uint32(reader, nread, "Failed to read thread id.")?;
        let stack_trace_id = read_var_uint32(reader, nread, "Failed to read stack trace id.")?;
        Ok(CPUEvent {
            thread_id,
            stack_trace_id,
        })
    }
}

struct AllocationEvent {
    thread_id: u32,
    stack_trace_id: u32,
    size: u64,
}

#[derive(Debug, Clone)]
struct MallocEvent {
    thread_id: u32,
    stack_trace_id: u32,
    address: u64,
    size: u64,
    allocator: u8,
}

impl Event for AllocationEvent {
    const ID: u8 = 33;

    fn parse(reader: &mut BufReader<File>, nread: &mut u64) -> Result<Self, &'static str> {
        let thread_id = read_var_uint32(reader, nread, "Failed to read thread id.")?;
        let stack_trace_id = read_var_uint32(reader, nread, "Failed to read stack trace id.")?;
        let size = read_var_uint64(reader, nread, "Failed to read size.")?;
        Ok(AllocationEvent {
            thread_id,
            stack_trace_id,
            size,
        })
    }
}

impl Event for MallocEvent {
    const ID: u8 = 34;

    fn parse(reader: &mut BufReader<File>, nread: &mut u64) -> Result<Self, &'static str> {
        let thread_id = read_var_uint32(reader, nread, "Failed to read thread id.")?;
        let stack_trace_id = read_var_uint32(reader, nread, "Failed to read stack trace id.")?;
        let address = read_var_uint64(reader, nread, "Failed to read address.")?;
        let size = read_var_uint64(reader, nread, "Failed to read size.")?;
        let allocator = read_uint8(reader, nread, "Failed to read allocator type.")?;
        Ok(MallocEvent {
            thread_id,
            stack_trace_id,
            address,
            size,
            allocator
        })
    }
}

#[derive(Eq, PartialEq, Hash)]
struct FunctionKey {
    function_name: u32,
    filename: u32,
    start_line: u32,
}

fn convert_to_string<'a>(raw: *const std::os::raw::c_char) -> Result<&'a str, ()> {
    if raw.is_null() {
        return Err(());
    }
    unsafe {
        match CStr::from_ptr(raw).to_str() {
            Ok(s) => Ok(s),
            Err(_) => {
                return Err(());
            }
        }
    }
}

fn write_content(output: &str, compress: bool, profile: &mut Profile) -> Result<(), String> {
    let file = File::create(output).map_err(|_| "Failed to create output file.")?;

    let mut writer: Box<dyn Write> = match compress {
        true => Box::new(GzEncoder::new(file, Compression::default())),
        false => Box::new(BufWriter::new(file)),
    };

    writer
        .write_all(&profile.encode_to_vec())
        .map_err(|_| "Failed to write profile.")?;

    Ok(())
}

fn do_convert_to_pprof(
    input: *const std::os::raw::c_char,
    cpu_output: *const std::os::raw::c_char,
    allocation_output: *const std::os::raw::c_char,
    native_memory_output: *const std::os::raw::c_char,
    native_memory_leaks: std::os::raw::c_uchar,
    compress: std::os::raw::c_uchar,
) -> Result<(), String> {
    let native_memory_leaks = match native_memory_leaks {
        1 => true,
        _ => false,
    };
    let compress = match compress {
        1 => true,
        _ => false,
    };

    let input = convert_to_string(input).map_err(|_| "Failed to parse input.")?;

    let cpu_output = convert_to_string(cpu_output).map_err(|_| "Failed to parse cpu output.")?;

    let allocation_output =
        convert_to_string(allocation_output).map_err(|_| "Failed to parse allocation output.")?;

    let native_memory_output =
        convert_to_string(native_memory_output).map_err(|_| "Failed to parse native memory output.")?;

    let mut parser = Parser::new(input);

    parser.parse().map_err(|e| e.to_string())?;

    let mut profile = Profile::default();
    let mut function_id_map: HashMap<FunctionKey, u32> = HashMap::new();
    let mut functions: Vec<Function> = Vec::new();
    let mut locations: Vec<Location> = Vec::new();
    let mut next_location_id = 1;

    let mut samples: Vec<Sample> = Vec::new();
    let mut stack_trace_id_to_sample_index: HashMap<u32, usize> = HashMap::new();

    for (stack_trace_id, stack_trace) in &parser.stack_traces {
        let mut lines: Vec<Line> = Vec::new();
        for frame in &stack_trace.frames {
            let function_key = FunctionKey {
                function_name: frame.name,
                filename: frame.filename,
                start_line: 0,
            };

            let function_id = if let Some(&id) = function_id_map.get(&function_key) {
                id
            } else {
                let id = functions.len() as u32 + 1;
                functions.push(Function {
                    id: id as u64,
                    name: frame.name as i64,
                    filename: frame.filename as i64,
                    ..Default::default()
                });
                function_id_map.insert(function_key, id);
                id
            };

            lines.push(Line {
                function_id: function_id as u64,
                line: frame.lineno as i64,
                ..Default::default()
            });
        }

        locations.push(Location {
            id: next_location_id,
            line: lines,
            ..Default::default()
        });

        samples.push(Sample {
            location_id: vec![next_location_id],
            value: vec![0, 0],
            ..Default::default()
        });

        stack_trace_id_to_sample_index.insert(*stack_trace_id, samples.len() - 1);

        next_location_id += 1;
    }

    profile.string_table = parser.string_table;
    profile.function = functions;
    profile.location = locations;
    profile.sample = samples;

    if parser.cpu_events.len() > 0 {
        for event in &parser.cpu_events {
            if let Some(&sample_index) = stack_trace_id_to_sample_index.get(&event.stack_trace_id) {
                profile.sample[sample_index].value[0] += 1;
                profile.sample[sample_index].value[1] += 10;
            }
        }

        profile.period_type = Some(ValueType { r#type: 1, unit: 2 });
        profile.sample_type = vec![
            ValueType { r#type: 5, unit: 6 },
            ValueType { r#type: 1, unit: 2 },
        ];

        write_content(cpu_output, compress, &mut profile)?;

        if parser.allocation_events.len() > 0 {
            for sample in &mut profile.sample {
                sample.value[0] = 0;
                sample.value[1] = 0;
            }
        }
    }

    if parser.allocation_events.len() > 0 {
        profile.period_type = Some(ValueType { r#type: 3, unit: 4 });
        profile.sample_type = vec![
            ValueType { r#type: 5, unit: 6 },
            ValueType { r#type: 3, unit: 4 },
        ];

        for event in &parser.allocation_events {
            if let Some(&sample_index) = stack_trace_id_to_sample_index.get(&event.stack_trace_id) {
                profile.sample[sample_index].value[0] += 1;
                profile.sample[sample_index].value[1] += event.size as i64;
            }
        }

        write_content(allocation_output, compress, &mut profile)?;
    }

    if parser.malloc_events.len() > 0 {
        let native_memory_events = if native_memory_leaks { find_leaks(parser.malloc_events) } else { parser.malloc_events };
        if native_memory_events.len() > 0 {
            profile.period_type = Some(ValueType { r#type: 3, unit: 4 });
            profile.sample_type = vec![
                ValueType { r#type: 5, unit: 6 },
                ValueType { r#type: 3, unit: 4 },
            ];
            for event in &native_memory_events {
                if let Some(&sample_index) = stack_trace_id_to_sample_index.get(&event.stack_trace_id) {
                    profile.sample[sample_index].value[0] += 1;
                    profile.sample[sample_index].value[1] += event.size as i64;
                }
            }

            write_content(native_memory_output, compress, &mut profile)?;
        }
    }
    Ok(())
}

fn find_leaks(events: Vec<MallocEvent>) -> Vec<MallocEvent> {
    let mut map: HashMap<u64, MallocEvent> = HashMap::new(); // Simple allocators: malloc/calloc/free/...
    let mut mmap_intervals: IntervalTree<MallocEvent> = IntervalTree::new() ; // RangedAllocators: mmap/munmap

    for e in events {
        let Some(alloc) = Allocator::from_u8(e.allocator) else {
            eprintln!("unknown allocator byte {} (event={:?})", e.allocator, e);
            continue;
        };

        match allocator_kind(alloc) {
            AllocatorKind::SimpleAllocator => {
                map.insert(e.address, e);
            }
            AllocatorKind::SimpleDeallocator => {
                map.remove(&e.address);
            }
            AllocatorKind::RangedAllocator => {
                mmap_intervals.add_interval(e.address, e.size, e);
            }
            AllocatorKind::RangedDeallocator => {
                mmap_intervals.remove_interval(e.address, e.size);
            }
        }
    }

    merge_events(&mmap_intervals, &map)
}

fn merge_events(
    tree: &IntervalTree<MallocEvent>,
    map: &HashMap<u64, MallocEvent>,
) -> Vec<MallocEvent>
where
    MallocEvent: Clone,
{
    let mut out = Vec::with_capacity(tree.iter().len() + map.len());

    // 1) take values from IntervalTree
    for (interval, ev) in tree.iter() {
        let mut e = ev.clone();
        e.size -= interval.size();
        out.push(e);
    }

    // 2) take values from HashMap
    for ev in map.values() {
        out.push(ev.clone());
    }

    out
}

#[unsafe(no_mangle)]
pub extern "C" fn convert_to_pprof(
    input: *const std::os::raw::c_char,
    cpu_output: *const std::os::raw::c_char,
    allocation_output: *const std::os::raw::c_char,
    native_memory_output: *const std::os::raw::c_char,
    native_memory_leaks: std::os::raw::c_uchar,
    compress: std::os::raw::c_uchar,
) -> i32 {
    match do_convert_to_pprof(input, cpu_output, allocation_output, native_memory_output, native_memory_leaks, compress) {
        Ok(_) => 0,
        Err(error_message) => {
            ERROR!("Failed to convert profile to pprof: {}", error_message);
            -1
        }
    }
}
