// Hand-rolled protobuf encoder for the Aliyun SLS log schema.
//
// Wire format reference:
//   https://developers.google.com/protocol-buffers/docs/encoding
//
// SLS log schema (`sls_logs.proto` — stable since 2014):
//   message Log {
//     required uint32 Time      = 1;   // seconds since epoch
//     message Content { required string Key = 1; required string Value = 2; }
//     repeated Content Contents = 2;
//     optional fixed32 Time_ns  = 4;   // omitted; we ship second resolution
//   }
//   message LogTag { required string Key = 1; required string Value = 2; }
//   message LogGroup {
//     repeated Log     Logs     = 1;
//     optional string  Reserved = 2;
//     optional string  Topic    = 3;
//     optional string  Source   = 4;
//     repeated LogTag  LogTags  = 6;
//   }

const WIRE_VARINT: u32 = 0;
const WIRE_LEN: u32 = 2;

#[derive(Debug, Clone)]
pub struct LogContent {
    pub key: String,
    pub value: String,
}

impl LogContent {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { key: key.into(), value: value.into() }
    }
}

#[derive(Debug, Clone)]
pub struct LogTag {
    pub key: String,
    pub value: String,
}

impl LogTag {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { key: key.into(), value: value.into() }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time_unix_secs: u32,
    pub contents: Vec<LogContent>,
}

impl LogEntry {
    pub fn new(time_unix_secs: u32, contents: Vec<LogContent>) -> Self {
        Self { time_unix_secs, contents }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LogGroup {
    pub logs: Vec<LogEntry>,
    pub topic: Option<String>,
    pub source: Option<String>,
    pub tags: Vec<LogTag>,
}

impl LogGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn add_log(&mut self, log: LogEntry) {
        self.logs.push(log);
    }

    pub fn add_tag(&mut self, tag: LogTag) {
        self.tags.push(tag);
    }
}

fn write_varint(buf: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

fn write_tag(buf: &mut Vec<u8>, field: u32, wire: u32) {
    write_varint(buf, ((field << 3) | wire) as u64);
}

fn write_len_delim(buf: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    write_tag(buf, field, WIRE_LEN);
    write_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

fn write_string(buf: &mut Vec<u8>, field: u32, value: &str) {
    write_len_delim(buf, field, value.as_bytes());
}

fn write_uint32(buf: &mut Vec<u8>, field: u32, value: u32) {
    write_tag(buf, field, WIRE_VARINT);
    write_varint(buf, value as u64);
}

fn encode_content(c: &LogContent) -> Vec<u8> {
    let mut buf = Vec::with_capacity(c.key.len() + c.value.len() + 8);
    write_string(&mut buf, 1, &c.key);
    write_string(&mut buf, 2, &c.value);
    buf
}

fn encode_log(log: &LogEntry) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16 + log.contents.len() * 16);
    write_uint32(&mut buf, 1, log.time_unix_secs);
    for c in &log.contents {
        let content_bytes = encode_content(c);
        write_len_delim(&mut buf, 2, &content_bytes);
    }
    buf
}

fn encode_tag(tag: &LogTag) -> Vec<u8> {
    let mut buf = Vec::with_capacity(tag.key.len() + tag.value.len() + 8);
    write_string(&mut buf, 1, &tag.key);
    write_string(&mut buf, 2, &tag.value);
    buf
}

/// Serialize a `LogGroup` to Aliyun SLS protobuf wire format.
///
/// The returned bytes are the uncompressed body; SLS PutLogs expects this
/// wrapped in LZ4 with `x-log-bodyrawsize` set to the pre-compression length.
pub fn encode_log_group(group: &LogGroup) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + group.logs.len() * 64);
    for log in &group.logs {
        let log_bytes = encode_log(log);
        write_len_delim(&mut buf, 1, &log_bytes);
    }
    if let Some(topic) = &group.topic {
        write_string(&mut buf, 3, topic);
    }
    if let Some(source) = &group.source {
        write_string(&mut buf, 4, source);
    }
    for tag in &group.tags {
        let tag_bytes = encode_tag(tag);
        write_len_delim(&mut buf, 6, &tag_bytes);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip_edges() {
        for v in [0u64, 1, 127, 128, 300, 16383, 16384, u32::MAX as u64] {
            let mut buf = Vec::new();
            write_varint(&mut buf, v);
            let (decoded, consumed) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v, "varint round-trip failed for {}", v);
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    fn encodes_minimal_log_group() {
        let mut group = LogGroup::new().with_topic("cf").with_source("host-1");
        group.add_log(LogEntry::new(
            1_700_000_000,
            vec![
                LogContent::new("level", "info"),
                LogContent::new("msg", "hello"),
            ],
        ));
        group.add_tag(LogTag::new("env", "test"));

        let bytes = encode_log_group(&group);
        let parsed = parse_log_group(&bytes).expect("decode round-trip");

        assert_eq!(parsed.topic.as_deref(), Some("cf"));
        assert_eq!(parsed.source.as_deref(), Some("host-1"));
        assert_eq!(parsed.tags.len(), 1);
        assert_eq!(parsed.tags[0].key, "env");
        assert_eq!(parsed.tags[0].value, "test");
        assert_eq!(parsed.logs.len(), 1);
        assert_eq!(parsed.logs[0].time_unix_secs, 1_700_000_000);
        assert_eq!(parsed.logs[0].contents.len(), 2);
        assert_eq!(parsed.logs[0].contents[0].key, "level");
        assert_eq!(parsed.logs[0].contents[0].value, "info");
        assert_eq!(parsed.logs[0].contents[1].key, "msg");
        assert_eq!(parsed.logs[0].contents[1].value, "hello");
    }

    #[test]
    fn encodes_empty_group() {
        let group = LogGroup::new();
        let bytes = encode_log_group(&group);
        assert!(bytes.is_empty(), "empty log group should encode to zero bytes");
    }

    #[test]
    fn encodes_many_logs() {
        let mut group = LogGroup::new();
        for i in 0..100u32 {
            group.add_log(LogEntry::new(
                1_700_000_000 + i,
                vec![LogContent::new("i", i.to_string())],
            ));
        }
        let bytes = encode_log_group(&group);
        let parsed = parse_log_group(&bytes).expect("decode round-trip");
        assert_eq!(parsed.logs.len(), 100);
        for (i, log) in parsed.logs.iter().enumerate() {
            assert_eq!(log.time_unix_secs, 1_700_000_000 + i as u32);
            assert_eq!(log.contents[0].value, i.to_string());
        }
    }

    // Minimal decoder used only in tests to verify wire-format correctness.

    fn decode_varint(buf: &[u8]) -> Option<(u64, usize)> {
        let mut value = 0u64;
        let mut shift = 0u32;
        for (i, b) in buf.iter().enumerate() {
            value |= ((*b & 0x7f) as u64) << shift;
            if *b & 0x80 == 0 {
                return Some((value, i + 1));
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
        None
    }

    fn decode_len(buf: &[u8]) -> Option<(&[u8], usize)> {
        let (len, consumed) = decode_varint(buf)?;
        let len = len as usize;
        if consumed + len > buf.len() {
            return None;
        }
        Some((&buf[consumed..consumed + len], consumed + len))
    }

    fn parse_content(mut buf: &[u8]) -> Option<LogContent> {
        let mut key = String::new();
        let mut value = String::new();
        while !buf.is_empty() {
            let (tag, c) = decode_varint(buf)?;
            buf = &buf[c..];
            let field = (tag >> 3) as u32;
            let wire = (tag & 0x7) as u32;
            match (field, wire) {
                (1, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    key = String::from_utf8(bytes.to_vec()).ok()?;
                    buf = &buf[c..];
                }
                (2, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    value = String::from_utf8(bytes.to_vec()).ok()?;
                    buf = &buf[c..];
                }
                _ => return None,
            }
        }
        Some(LogContent { key, value })
    }

    fn parse_log(mut buf: &[u8]) -> Option<LogEntry> {
        let mut time = 0u32;
        let mut contents = Vec::new();
        while !buf.is_empty() {
            let (tag, c) = decode_varint(buf)?;
            buf = &buf[c..];
            let field = (tag >> 3) as u32;
            let wire = (tag & 0x7) as u32;
            match (field, wire) {
                (1, 0) => {
                    let (v, c) = decode_varint(buf)?;
                    time = v as u32;
                    buf = &buf[c..];
                }
                (2, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    contents.push(parse_content(bytes)?);
                    buf = &buf[c..];
                }
                _ => return None,
            }
        }
        Some(LogEntry { time_unix_secs: time, contents })
    }

    fn parse_tag(mut buf: &[u8]) -> Option<LogTag> {
        let mut key = String::new();
        let mut value = String::new();
        while !buf.is_empty() {
            let (tag, c) = decode_varint(buf)?;
            buf = &buf[c..];
            let field = (tag >> 3) as u32;
            let wire = (tag & 0x7) as u32;
            match (field, wire) {
                (1, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    key = String::from_utf8(bytes.to_vec()).ok()?;
                    buf = &buf[c..];
                }
                (2, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    value = String::from_utf8(bytes.to_vec()).ok()?;
                    buf = &buf[c..];
                }
                _ => return None,
            }
        }
        Some(LogTag { key, value })
    }

    fn parse_log_group(mut buf: &[u8]) -> Option<LogGroup> {
        let mut group = LogGroup::new();
        while !buf.is_empty() {
            let (tag, c) = decode_varint(buf)?;
            buf = &buf[c..];
            let field = (tag >> 3) as u32;
            let wire = (tag & 0x7) as u32;
            match (field, wire) {
                (1, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    group.logs.push(parse_log(bytes)?);
                    buf = &buf[c..];
                }
                (3, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    group.topic = Some(String::from_utf8(bytes.to_vec()).ok()?);
                    buf = &buf[c..];
                }
                (4, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    group.source = Some(String::from_utf8(bytes.to_vec()).ok()?);
                    buf = &buf[c..];
                }
                (6, 2) => {
                    let (bytes, c) = decode_len(buf)?;
                    group.tags.push(parse_tag(bytes)?);
                    buf = &buf[c..];
                }
                _ => return None,
            }
        }
        Some(group)
    }
}
