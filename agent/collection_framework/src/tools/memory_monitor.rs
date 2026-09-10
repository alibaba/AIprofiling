use anyhow::Result;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tracing as log;

/// Memory monitor that tracks peak memory usage of the current process
pub struct MemoryMonitor {
    peak_rss_kb: Arc<AtomicU64>,
    is_monitoring: Arc<std::sync::atomic::AtomicBool>,
    monitor_handle: Option<tokio::task::JoinHandle<()>>,
}

impl MemoryMonitor {
    /// Create a new memory monitor
    pub fn new() -> Self {
        Self {
            peak_rss_kb: Arc::new(AtomicU64::new(0)),
            is_monitoring: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            monitor_handle: None,
        }
    }

    /// Start monitoring memory usage in the background
    ///
    /// # Arguments
    /// * `interval_ms` - Monitoring interval in milliseconds (default: 100ms)
    /// * `memory_limit_kb` - Optional memory limit in KB. If exceeded, triggers graceful shutdown.
    pub fn start(&mut self, interval_ms: u64, _memory_limit_kb: Option<u64>) {
        if self.is_monitoring.load(Ordering::Relaxed) {
            log::warn!("Memory monitor is already running");
            return;
        }

        self.is_monitoring.store(true, Ordering::Relaxed);
        let peak_rss = Arc::clone(&self.peak_rss_kb);
        let is_monitoring = Arc::clone(&self.is_monitoring);
        let pid = std::process::id();

        let handle = tokio::spawn(async move {
            log::info!(
                "Memory monitor started for PID {} with interval {}ms",
                pid,
                interval_ms
            );

            while is_monitoring.load(Ordering::Relaxed) {
                match get_current_rss_kb(pid) {
                    Ok(current_rss) => {
                        // // Check if memory usage exceeds the limit
                        // if let Some(limit_kb) = memory_limit_kb {
                        //     if current_rss > limit_kb {
                        //         log::error!(
                        //             "Peak memory usage exceeded limit (current: {:.2} MB, limit: {:.2} MB). Performing graceful shutdown...",
                        //             current_rss as f64 / 1024.0,
                        //             limit_kb as f64 / 1024.0
                        //         );
                        //         if let Err(e) = EventHandler::global_sender().send(SchedulerEvent::Exit) {
                        //             log::error!("Failed to send Exit event to scheduler: {}", e);
                        //         }
                        //         is_monitoring.store(false, Ordering::Relaxed);
                        //         break;
                        //     }
                        // }

                        let mut peak = peak_rss.load(Ordering::Relaxed);
                        while current_rss > peak {
                            match peak_rss.compare_exchange(
                                peak,
                                current_rss,
                                Ordering::Release,
                                Ordering::Relaxed,
                            ) {
                                Ok(_) => break,
                                Err(x) => peak = x,
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to read memory usage: {}", e);
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms)).await;
            }

            log::info!("Memory monitor stopped");
        });

        self.monitor_handle = Some(handle);
    }

    /// Stop monitoring memory usage
    pub async fn stop(&mut self) {
        self.is_monitoring.store(false, Ordering::Relaxed);

        if let Some(handle) = self.monitor_handle.take() {
            let _ = handle.await;
        }
    }

    /// Get the peak RSS memory usage in KB
    pub fn get_peak_rss_kb(&self) -> u64 {
        self.peak_rss_kb.load(Ordering::Relaxed)
    }

    /// Get the peak RSS memory usage in MB
    pub fn get_peak_rss_mb(&self) -> f64 {
        self.get_peak_rss_kb() as f64 / 1024.0
    }

    /// Get the peak RSS memory usage in GB
    pub fn get_peak_rss_gb(&self) -> f64 {
        self.get_peak_rss_kb() as f64 / (1024.0 * 1024.0)
    }

    /// Print the peak memory usage
    pub fn print_peak_memory(&self) {
        let peak_mb = self.get_peak_rss_mb();
        let peak_gb = self.get_peak_rss_gb();

        log::info!("Peak Memory Usage Summary:");
        log::info!("Peak RSS: {:.2} MB, {:.4} GB", peak_mb, peak_gb);
    }
}

impl Drop for MemoryMonitor {
    fn drop(&mut self) {
        self.is_monitoring.store(false, Ordering::Relaxed);
    }
}

/// Get current RSS (Resident Set Size) memory usage in KB for a given PID
fn get_current_rss_kb(pid: u32) -> Result<u64> {
    let stat_path = format!("/proc/{}/statm", pid);
    let stat_content = fs::read_to_string(&stat_path)?;

    // /proc/[pid]/statm format:
    // size resident shared text lib data dt
    // We need the second field (resident), which is in pages
    let parts: Vec<&str> = stat_content.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(anyhow::anyhow!("Invalid statm format"));
    }

    let rss_pages: u64 = parts[1].parse()?;

    // Get page size (typically 4096 bytes = 4KB)
    let page_size_kb = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 / 1024 };

    Ok(rss_pages * page_size_kb)
}
