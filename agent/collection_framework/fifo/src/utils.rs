use std::io::{Error, ErrorKind, Result};

// const TS_MAGIC: u64 = 1_000_000_000 * 10_000_000; //* 60 * 60 * 24 * 365; // 1 year in nanoseconds

pub(crate) fn parse_ts(start: u64, ts_off: u64) -> Result<f64> {
    // filter out the ts equal to 0 or std::u64::MAX, that means the ts data is corrupted
    if start == 0 || start == std::u64::MAX {
        Err(Error::new(
            ErrorKind::InvalidData,
            format!("timestamp is {}", start),
        ))
    } else {
        Ok(start.saturating_sub(ts_off) as f64 / 1000.0)
    }
}
