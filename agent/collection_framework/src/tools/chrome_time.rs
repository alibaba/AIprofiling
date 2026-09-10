use std::time::{SystemTime, UNIX_EPOCH};

/// 3 months in seconds = 3 * 30.44 * 24 * 60 * 60 ≈ 7889238
const THREE_MONTHS_SECONDS: u64 = 7889238;

/// A struct to provide base time for Chrome tracing, making all timestamps relative to 3-month intervals
pub struct ChromeTraceBaseTime;

impl ChromeTraceBaseTime {
    /// Returns the base time in nanoseconds
    pub fn get_base_time() -> i64 {
        // Make all timestamps relative to 3 month intervals.
        static mut BASE_TIME: i64 = 0;
        static INIT: std::sync::Once = std::sync::Once::new();

        unsafe {
            INIT.call_once(|| {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("Time went backwards")
                    .as_secs();

                // Calculate the start of the current 3-month interval
                let base_interval = (now / THREE_MONTHS_SECONDS) * THREE_MONTHS_SECONDS;
                BASE_TIME = (base_interval * 1_000_000_000) as i64;
            });

            BASE_TIME
        }
    }
}
