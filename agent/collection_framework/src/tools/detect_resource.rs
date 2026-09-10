use crate::config::Config;
use crate::error::ErrorCode;
use anyhow::Result;
use log::debug;
use std::fs;
use std::process::Command;
use tracing as log;

use crate::tools::utils::is_process_in_container;

/// Check whether system resources meet collection requirements.
/// If CPU usage is too high or free memory is too low, do not start the collector.
pub fn detect_resource(config: &Config, pid: &i32) -> Result<bool> {
    // Iterate over resource configuration
    for resource in &config.resources {
        for dependency in &resource.dependencies {
            let result = match dependency.detector.as_str() {
                "detect_cpu_support" => {
                    // CPU support detection is disabled for now; will be enabled once
                    // the concrete detection criteria are agreed upon.
                    // detect_cpu_support(dependency, pid)
                    Ok(())
                }
                "detect_mem_support" => detect_mem_support(dependency, pid),
                _ => {
                    return Err(ErrorCode::DetectEnvError(None)
                        .with_details(format!(
                            "Unknown resource detector: {}",
                            dependency.detector
                        ))
                        .into_error());
                }
            };

            // If any check fails, the process does not meet requirements
            if result.is_err() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Check whether CPU usage exceeds the threshold
fn detect_cpu_support(dependency: &crate::config::DependencyConfig, pid: &i32) -> Result<()> {
    // Get the CPU usage threshold
    let threshold = dependency
        .params
        .get("threshold")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ErrorCode::DetectEnvError(None)
                .with_details("Missing threshold parameter for CPU detection".to_string())
                .into_error()
        })?;

    // Parse the threshold operator and value
    let (operator, value_str) = parse_threshold(threshold)?;
    let threshold_value: f32 = value_str.parse().map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Invalid threshold value for CPU detection".to_string())
            .into_error()
    })?;

    // Get current CPU usage
    let current_cpu_usage = get_cpu_usage(pid)?;

    // Check CPU usage against the operator
    let should_abort = match operator {
        ">" => current_cpu_usage > threshold_value,
        ">=" => current_cpu_usage >= threshold_value,
        "<" => current_cpu_usage < threshold_value,
        "<=" => current_cpu_usage <= threshold_value,
        "=" | "==" => (current_cpu_usage - threshold_value).abs() < 0.01,
        _ => {
            return Err(ErrorCode::DetectEnvError(None)
                .with_details("Invalid operator for CPU threshold".to_string())
                .into_error());
        }
    };

    if should_abort {
        log::error!(
            "Process {} CPU usage check failed - Current: {:.2}%, Threshold: {}{}%",
            pid,
            current_cpu_usage,
            operator,
            threshold_value
        );
        return Err(ErrorCode::DetectEnvError(None)
            .with_details(format!(
                "CPU usage {:.2}% exceeds threshold {}{}%",
                current_cpu_usage, operator, threshold_value
            ))
            .into_error());
    }

    log::info!(
        "Process {} CPU usage check passed - Current: {:.2}%, Threshold: {}{}%",
        pid,
        current_cpu_usage,
        operator,
        threshold_value
    );

    Ok(())
}

/// Check whether free memory is below the threshold
fn detect_mem_support(dependency: &crate::config::DependencyConfig, pid: &i32) -> Result<()> {
    // Get the memory threshold
    let threshold = dependency
        .params
        .get("threshold")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ErrorCode::DetectEnvError(None)
                .with_details("Missing threshold parameter for memory detection".to_string())
                .into_error()
        })?;

    log::debug!(
        "Detecting memory support for process {} with threshold {}",
        pid,
        threshold
    );
    // Get the unit; configurations like "GB/s/proc" are supported, but only the
    // first unit (e.g. GB) is used here.
    let raw_unit = dependency
        .params
        .get("unit")
        .and_then(|v| v.as_str())
        .unwrap_or("GB");
    let unit = raw_unit.split('/').next().unwrap_or(raw_unit);

    // Parse the threshold operator and value
    let (operator, value_str) = parse_threshold(threshold)?;
    let threshold_value: f32 = value_str.parse().map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Invalid threshold value for memory detection".to_string())
            .into_error()
    })?;

    // Get current available memory (in GB)
    let available_memory_gb = get_available_memory_gb(pid)?;

    // Convert the threshold to GB
    let threshold_value_gb = match unit.to_uppercase().as_str() {
        "GB" => threshold_value,
        "MB" => threshold_value / 1024.0,
        "KB" => threshold_value / (1024.0 * 1024.0),
        _ => {
            return Err(ErrorCode::DetectEnvError(None)
                .with_details("Unsupported memory unit".to_string())
                .into_error());
        }
    };

    // Check memory against the operator
    let should_abort = match operator {
        "<" => available_memory_gb < threshold_value_gb,
        "<=" => available_memory_gb <= threshold_value_gb,
        ">" => available_memory_gb > threshold_value_gb,
        ">=" => available_memory_gb >= threshold_value_gb,
        "=" | "==" => (available_memory_gb - threshold_value_gb).abs() < 0.01,
        _ => {
            return Err(ErrorCode::DetectEnvError(None)
                .with_details("Invalid operator for memory threshold".to_string())
                .into_error());
        }
    };

    if should_abort {
        log::error!(
            "Process {} memory check failed - Available: {:.2}GB, Threshold: {}{}{}",
            pid,
            available_memory_gb,
            operator,
            threshold_value,
            unit
        );
        return Err(ErrorCode::DetectEnvError(None)
            .with_details(format!(
                "Available memory {:.2}GB is below threshold {}{}{}",
                available_memory_gb, operator, threshold_value, unit
            ))
            .into_error());
    }

    log::debug!(
        "Process {} memory check passed - Available: {:.2}GB, Threshold: {}{}{}",
        pid,
        available_memory_gb,
        operator,
        threshold_value,
        unit
    );

    Ok(())
}

/// Parse a threshold string and return the operator and value
fn parse_threshold(threshold: &str) -> Result<(&str, &str)> {
    let trimmed = threshold.trim();

    // Supported operators (sorted by length so longer matches take precedence)
    let operators = [">=", "<=", "==", ">", "<", "="];

    for op in &operators {
        if trimmed.starts_with(op) {
            let value = trimmed.trim_start_matches(op).trim();
            return Ok((*op, value));
        }
    }

    // Default to the equality operator
    Ok(("=", trimmed))
}

/// Get current CPU usage (percentage)
fn get_cpu_usage(pid: &i32) -> Result<f32> {
    // Three-level decision:
    // 1. First check whether the process itself has a resource limit; if so, use
    //    the process-level available CPU.
    // 2. If the process has no limit and it runs inside a container, check the
    //    container's resource limit and current usage.
    // 3. If the container has no limit, fall back to the host.

    // Try to read the process-level CPU limit (via cgroup)
    if let Ok(process_cpu_usage) = get_process_cpu_usage(pid) {
        return Ok(process_cpu_usage);
    }

    // Process has no limit; check whether it is in a container
    if is_process_in_container(*pid) {
        // In a container: try to read the container-level CPU limit
        debug!("Process is in container, trying to get container CPU usage");
        if let Ok(container_cpu_usage) = get_container_cpu_usage(pid) {
            return Ok(container_cpu_usage);
        }
    }

    // Container has no limit or process is not in a container; use host resources
    let stat_content = fs::read_to_string("/proc/stat")?;
    parse_cpu_usage_from_content(&stat_content)
}

/// Get process-level CPU usage (via prlimit)
fn get_process_cpu_usage(pid: &i32) -> Result<f32> {
    // Use prlimit to check whether the process has a CPU time limit
    let output = Command::new("prlimit")
        .arg("--pid")
        .arg(pid.to_string())
        .output()
        .map_err(|e| {
            ErrorCode::DetectEnvError(None)
                .with_details(format!("Failed to execute prlimit: {}", e))
                .into_error()
        })?;

    if !output.status.success() {
        return Err(ErrorCode::DetectEnvError(None)
            .with_details("Failed to read CPU limit info".to_string())
            .into_error());
    }

    let content = String::from_utf8(output.stdout).map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Failed to parse prlimit info".to_string())
            .into_error()
    })?;

    // Parse output and find the CPU line
    for line in content.lines() {
        if line.starts_with("CPU") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let soft_limit = parts[3];
                let hard_limit = parts[4];
                debug!("CPU soft limit: {}, hard limit: {}", soft_limit, hard_limit);
                // Check whether a limit exists (not "unlimited")
                if soft_limit != "unlimited" {
                    // Parse the CPU usage limit
                    let cpu_limit: f32 = soft_limit.parse().map_err(|_| {
                        ErrorCode::DetectEnvError(None)
                            .with_details("Failed to parse CPU limit value".to_string())
                            .into_error()
                    })?;

                    return Ok(cpu_limit);
                } else if hard_limit != "unlimited" {
                    // Soft limit is unlimited but the hard limit is set
                    let cpu_limit: f32 = hard_limit.parse().map_err(|_| {
                        ErrorCode::DetectEnvError(None)
                            .with_details("Failed to parse CPU limit value".to_string())
                            .into_error()
                    })?;

                    return Ok(cpu_limit);
                } else {
                    // No limit; return an error to indicate "unlimited"
                    debug!("No CPU limit found in prlimit command");
                    return Err(ErrorCode::DetectEnvError(None)
                        .with_details("No CPU limit found".to_string())
                        .into_error());
                }
            }
        }
    }

    // No CPU line found; return an error
    Err(ErrorCode::DetectEnvError(None)
        .with_details("Failed to find CPU limit info".to_string())
        .into_error())
}

/// Get container CPU usage
fn get_container_cpu_usage(pid: &i32) -> Result<f32> {
    // Read the process's cgroup info directly
    let cgroup_path = format!("/proc/{}/cgroup", pid);
    let cgroup_content = fs::read_to_string(&cgroup_path).map_err(|e| {
        ErrorCode::DetectEnvError(None)
            .with_details(format!("Failed to read cgroup info: {}", e))
            .into_error()
    })?;

    // Find the cgroup path for cpu or cpuacct
    let mut cpu_cgroup_path = String::new();
    for line in cgroup_content.lines() {
        // Look for lines containing cpu, cpuacct or cpuset
        if line.contains(":cpu,") || line.contains(":cpuacct,") || line.contains("cpu,cpuacct") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 {
                cpu_cgroup_path = parts[2].to_string();
                // Ensure the path starts with '/'
                if !cpu_cgroup_path.starts_with('/') {
                    cpu_cgroup_path = format!("/{}", cpu_cgroup_path);
                }
                break;
            }
        }
    }
    debug!("cpu_cgroup_path: {}", cpu_cgroup_path);
    // If no cpu/cpuacct cgroup was found, try lines containing "cpu"
    if cpu_cgroup_path.is_empty() {
        for line in cgroup_content.lines() {
            if line.contains(":cpu:") {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 3 {
                    cpu_cgroup_path = parts[2].to_string();
                    if !cpu_cgroup_path.starts_with('/') {
                        cpu_cgroup_path = format!("/{}", cpu_cgroup_path);
                    }
                    break;
                }
            }
        }
    }

    if cpu_cgroup_path.is_empty() {
        // No cpu/cpuacct cgroup found; fall back to /proc/stat
        let stat_path = format!("/proc/{}/stat", pid);
        let stat_content = fs::read_to_string(&stat_path).map_err(|e| {
            ErrorCode::DetectEnvError(None)
                .with_details(format!("Failed to read CPU stats: {}", e))
                .into_error()
        })?;

        return parse_cpu_usage_from_content(&stat_content);
    }

    // Build paths for the CPU usage files
    let cpu_usage_path = format!("/sys/fs/cgroup/cpu{}/cpuacct.usage", cpu_cgroup_path);
    let cpu_period_path = format!("/sys/fs/cgroup/cpu{}/cpu.cfs_period_us", cpu_cgroup_path);
    let cpu_quota_path = format!("/sys/fs/cgroup/cpu{}/cpu.cfs_quota_us", cpu_cgroup_path);
    // debug!("cpu_usage_path: {}, cpu_period_path: {}, cpu_quota_path: {}", cpu_usage_path, cpu_period_path, cpu_quota_path);
    // Read CPU usage
    let cpu_usage_content = fs::read_to_string(&cpu_usage_path).map_err(|e| {
        ErrorCode::DetectEnvError(None)
            .with_details(format!("Failed to read cpu usage: {}", e))
            .into_error()
    })?;

    let cpu_period_content = fs::read_to_string(&cpu_period_path).map_err(|e| {
        ErrorCode::DetectEnvError(None)
            .with_details(format!("Failed to read cpu period: {}", e))
            .into_error()
    })?;

    let cpu_quota_content = fs::read_to_string(&cpu_quota_path).map_err(|e| {
        ErrorCode::DetectEnvError(None)
            .with_details(format!("Failed to read cpu quota: {}", e))
            .into_error()
    })?;

    // Parse CPU usage
    let cpu_usage_content = cpu_usage_content.trim().to_string();
    let cpu_period_content = cpu_period_content.trim().to_string();
    let cpu_quota_content = cpu_quota_content.trim().to_string();

    let cpu_usage: u64 = cpu_usage_content.parse().unwrap_or(0);
    let cpu_period: u64 = cpu_period_content.parse().unwrap_or(100000);
    let cpu_quota: i64 = cpu_quota_content.parse().unwrap_or(-1);

    // Compute the CPU usage percentage
    if cpu_quota > 0 {
        // A CPU limit is set
        let cpu_percentage = (cpu_usage as f32) / (cpu_period as f32) / (cpu_quota as f32) * 100.0;
        Ok(cpu_percentage)
    } else {
        // No CPU limit; use the total number of cores
        let num_cores = num_cpus::get() as f32;
        debug!("num_cores: {}", num_cores);
        let cpu_percentage = (cpu_usage as f32) / 1_000_000_000.0 / num_cores * 100.0;
        Ok(cpu_percentage)
    }
}

/// Parse CPU usage
fn parse_cpu_usage_from_content(stat_content: &str) -> Result<f32> {
    // Read the first line (aggregate CPU info)
    let cpu_line = stat_content
        .lines()
        .next()
        .ok_or_else(|| {
            ErrorCode::DetectEnvError(None)
                .with_details("Failed to read CPU stats".to_string())
                .into_error()
        })?
        .trim();

    // Parse CPU statistics
    let parts: Vec<&str> = cpu_line.split_whitespace().collect();
    if parts.len() < 5 {
        return Err(ErrorCode::DetectEnvError(None)
            .with_details("Invalid CPU stats format".to_string())
            .into_error());
    }

    // Compute CPU usage
    // user + nice + system + idle + iowait + irq + softirq
    let user: u64 = parts[1].parse().unwrap_or(0);
    let nice: u64 = parts[2].parse().unwrap_or(0);
    let system: u64 = parts[3].parse().unwrap_or(0);
    let idle: u64 = parts[4].parse().unwrap_or(0);
    let iowait: u64 = parts.get(5).unwrap_or(&"0").parse().unwrap_or(0);
    let irq: u64 = parts.get(6).unwrap_or(&"0").parse().unwrap_or(0);
    let softirq: u64 = parts.get(7).unwrap_or(&"0").parse().unwrap_or(0);

    let total = user + nice + system + idle + iowait + irq + softirq;
    let active = user + nice + system + irq + softirq;
    let _idle_total = idle + iowait;

    if total == 0 {
        return Ok(0.0);
    }

    // CPU usage = (total - idle) / total * 100
    let usage = ((active as f32) / (total as f32)) * 100.0;
    debug!("CPU usage: {}", usage);
    Ok(usage)
}

/// Get available memory (in GB)
fn get_available_memory_gb(pid: &i32) -> Result<f32> {
    // Three-level decision:
    // 1. First check whether the process itself has a resource limit; if so, use
    //    the process-level available memory.
    // 2. If the process has no limit and it runs inside a container, check the
    //    container's resource limit and current usage.
    // 3. If the container has no limit, fall back to the host.

    // Try to read the process-level memory limit (via cgroup)
    if let Ok(process_memory_gb) = get_process_memory_gb(pid) {
        log::debug!("Available memory in process: {} GB", process_memory_gb);
        return Ok(process_memory_gb);
    }

    // Process has no limit; check whether it is in a container
    if is_process_in_container(*pid) {
        // In a container: try to read the container-level memory limit
        if let Ok(container_memory_gb) = get_container_memory_gb(pid) {
            debug!("Available memory in container: {} GB", container_memory_gb);
            return Ok(container_memory_gb);
        }
    }
    log::debug!("checking host memory..");
    // Container has no limit or process is not in a container; use host resources
    get_host_available_memory_gb()
}

/// Get host available memory (in GB)
fn get_host_available_memory_gb() -> Result<f32> {
    // Read /proc/meminfo for memory info
    let meminfo_content = fs::read_to_string("/proc/meminfo").map_err(|e| {
        ErrorCode::DetectEnvError(None)
            .with_details(format!("Failed to read meminfo: {}", e))
            .into_error()
    })?;

    // Look for the MemAvailable line
    for line in meminfo_content.lines() {
        if line.starts_with("MemAvailable:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let available_kb: u64 = parts[1].parse().unwrap_or(0);
                // Convert to GB (1 GB = 1024 * 1024 KB)
                return Ok((available_kb as f32) / (1024.0 * 1024.0));
            }
        }
    }

    // Fall back to MemFree if MemAvailable is not present
    for line in meminfo_content.lines() {
        if line.starts_with("MemFree:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let free_kb: u64 = parts[1].parse().unwrap_or(0);
                // Convert to GB (1 GB = 1024 * 1024 KB)
                return Ok((free_kb as f32) / (1024.0 * 1024.0));
            }
        }
    }

    Err(ErrorCode::DetectEnvError(None)
        .with_details("Failed to read memory information".to_string())
        .into_error())
}

/// Get process-level memory usage (via prlimit)
fn get_process_memory_gb(pid: &i32) -> Result<f32> {
    // Use prlimit to check the process's address-space limit
    let output = Command::new("prlimit")
        .arg("--pid")
        .arg(pid.to_string())
        .output()
        .map_err(|e| {
            ErrorCode::DetectEnvError(None)
                .with_details(format!("Failed to execute prlimit: {}", e))
                .into_error()
        })?;

    if !output.status.success() {
        return Err(ErrorCode::DetectEnvError(None)
            .with_details("Failed to read memory limit info".to_string())
            .into_error());
    }

    let content = String::from_utf8(output.stdout).map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Failed to parse prlimit info".to_string())
            .into_error()
    })?;

    // Parse output; find the AS line (address-space limit)
    let mut as_soft_limit = "unlimited";
    let mut as_hard_limit = "unlimited";

    for line in content.lines() {
        if line.starts_with("AS") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 5 {
                as_soft_limit = parts[3];
                as_hard_limit = parts[4];
                debug!(
                    "AS soft limit: {}, AS hard limit: {}",
                    as_soft_limit, as_hard_limit
                );
                break;
            }
        }
    }

    // Check whether the address-space limit is actually set
    if as_soft_limit == "unlimited" && as_hard_limit == "unlimited" {
        // No limit; return an error indicating "unlimited"
        return Err(ErrorCode::DetectEnvError(None)
            .with_details("No memory limit found".to_string())
            .into_error());
    }

    // Parse the address-space limit (in bytes)
    let as_limit_str = if as_soft_limit != "unlimited" {
        as_soft_limit
    } else {
        as_hard_limit
    };

    let as_limit: u64 = as_limit_str.parse().map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Failed to parse address space limit".to_string())
            .into_error()
    })?;

    // Get the process's RSS (physical memory currently in use)
    let stat_path = format!("/proc/{}/stat", pid);
    let stat_content = fs::read_to_string(&stat_path).map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Failed to read process stat".to_string())
            .into_error()
    })?;

    let stat_parts: Vec<&str> = stat_content.split_whitespace().collect();
    if stat_parts.len() < 24 {
        return Err(ErrorCode::DetectEnvError(None)
            .with_details("Invalid process stat format".to_string())
            .into_error());
    }

    // RSS is field 24 (index 23), in pages (typically 4KB each)
    let rss_pages: u64 = stat_parts[23].parse().map_err(|_| {
        ErrorCode::DetectEnvError(None)
            .with_details("Failed to parse RSS value".to_string())
            .into_error()
    })?;

    let page_size = 4096u64; // Page size is typically 4KB
    let rss_bytes = rss_pages * page_size;

    // Compute available memory (in GB)
    let available_memory = as_limit.saturating_sub(rss_bytes);
    let available_memory_gb = (available_memory as f32) / (1024.0 * 1024.0 * 1024.0);

    Ok(available_memory_gb)
}

/// Get container memory usage (in GB)
fn get_container_memory_gb(pid: &i32) -> Result<f32> {
    // Read the process's cgroup info directly
    let cgroup_path = format!("/proc/{}/cgroup", pid);
    let cgroup_content = fs::read_to_string(&cgroup_path).map_err(|e| {
        ErrorCode::DetectEnvError(None)
            .with_details(format!("Failed to read cgroup info: {}", e))
            .into_error()
    })?;

    // Find the memory cgroup path
    let mut memory_cgroup_path = String::new();
    let mut is_v2 = false;

    // Detect cgroup v2 (a single line starting with "0::")
    let lines: Vec<&str> = cgroup_content.lines().collect();
    if lines.len() == 1 && lines[0].starts_with("0::") {
        // cgroup v2
        is_v2 = true;
        let parts: Vec<&str> = lines[0].splitn(3, ':').collect();
        if parts.len() == 3 {
            memory_cgroup_path = parts[2].to_string();
        }
    } else {
        // cgroup v1
        for line in cgroup_content.lines() {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 3 && line.contains(":memory:") {
                memory_cgroup_path = parts[2].to_string();
                // Ensure the path starts with '/'
                if !memory_cgroup_path.starts_with('/') {
                    memory_cgroup_path = format!("/{}", memory_cgroup_path);
                }
                break;
            }
        }
    }

    // Normalize the relative-path form seen in Kubernetes environments;
    // repeatedly collapse any "/../" segment.
    while memory_cgroup_path.contains("/../") {
        memory_cgroup_path = memory_cgroup_path.replacen("/../", "/", 1);
    }

    if memory_cgroup_path.is_empty() {
        // No memory cgroup found; fall back to /proc/meminfo
        let meminfo_path = format!("/proc/{}/meminfo", pid);
        let meminfo_content = fs::read_to_string(&meminfo_path).map_err(|e| {
            ErrorCode::DetectEnvError(None)
                .with_details(format!("Failed to read meminfo: {}", e))
                .into_error()
        })?;

        // Look for the MemAvailable line
        for line in meminfo_content.lines() {
            if line.starts_with("MemAvailable:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let available_kb: u64 = parts[1].parse().unwrap_or(0);
                    // Convert to GB (1 GB = 1024 * 1024 KB)
                    return Ok((available_kb as f32) / (1024.0 * 1024.0));
                }
            }
        }

        // Fall back to MemFree if MemAvailable is not present
        for line in meminfo_content.lines() {
            if line.starts_with("MemFree:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let free_kb: u64 = parts[1].parse().unwrap_or(0);
                    // Convert to GB (1 GB = 1024 * 1024 KB)
                    return Ok((free_kb as f32) / (1024.0 * 1024.0));
                }
            }
        }

        return Err(ErrorCode::DetectEnvError(None)
            .with_details("Failed to read memory information in container".to_string())
            .into_error());
    }

    if is_v2 {
        // cgroup v2 branch
        let memory_current_path = format!(
            "/sys/fs/cgroup{}memory.current",
            if memory_cgroup_path.ends_with('/') {
                memory_cgroup_path.clone()
            } else {
                format!("{}/", memory_cgroup_path)
            }
        );

        let memory_max_path = format!(
            "/sys/fs/cgroup{}memory.max",
            if memory_cgroup_path.ends_with('/') {
                memory_cgroup_path.clone()
            } else {
                format!("{}/", memory_cgroup_path)
            }
        );

        // Read current memory usage
        let memory_current_content = fs::read_to_string(&memory_current_path);
        let current_memory: i64 = match memory_current_content {
            Ok(content) => content.trim().parse().unwrap_or(0),
            Err(_) => 0,
        };

        // Read the memory limit
        let memory_max_content = fs::read_to_string(&memory_max_path);
        let max_memory: i64 = match memory_max_content {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed == "max" {
                    // No limit; fall back to host memory
                    return get_host_available_memory_gb();
                }
                trimmed.parse().unwrap_or(i64::MAX)
            }
            Err(_) => i64::MAX,
        };

        if max_memory >= i64::MAX - 1000000 {
            // No limit; fall back to host memory
            return get_host_available_memory_gb();
        }

        // Compute available memory (in GB)
        let available_memory = max_memory.saturating_sub(current_memory);
        let available_memory_gb = (available_memory as f32) / (1024.0 * 1024.0 * 1024.0);
        return Ok(available_memory_gb);
    } else {
        // cgroup v1 branch
        // Build paths for the memory-limit files
        let memory_limit_path = format!(
            "/sys/fs/cgroup/memory{}/memory.limit_in_bytes",
            memory_cgroup_path
        );
        let memory_usage_path = format!(
            "/sys/fs/cgroup/memory{}/memory.usage_in_bytes",
            memory_cgroup_path
        );

        // Read the memory limit and current usage
        let memory_limit_content = fs::read_to_string(&memory_limit_path);

        // If the current level has no memory limit, walk up to a parent's limit
        let (effective_memory_limit, effective_memory_usage) =
            if let Ok(limit_content) = &memory_limit_content {
                let limit: i64 = limit_content.trim().parse().unwrap_or(i64::MAX);
                // If the value is huge (near i64::MAX), keep looking upward
                if limit < i64::MAX - 1000000 {
                    let usage_content = fs::read_to_string(&memory_usage_path).map_err(|e| {
                        ErrorCode::DetectEnvError(None)
                            .with_details(format!("Failed to read memory usage: {}", e))
                            .into_error()
                    })?;
                    let usage: i64 = usage_content.trim().parse().unwrap_or(0);
                    (limit, usage)
                } else {
                    // No limit at this level; look for one on a parent directory
                    find_parent_memory_limit(&memory_cgroup_path)?
                }
            } else {
                // Cannot read the limit here; look for one on a parent directory
                find_parent_memory_limit(&memory_cgroup_path)?
            };

        // If the value is huge (near i64::MAX), fall back to host memory.
        // Some container environments report a sentinel like i64::MAX-1 when no
        // limit is set.
        if effective_memory_limit >= i64::MAX - 1000000 {
            // Leave some slack
            // Use host available memory
            return get_host_available_memory_gb();
        }

        // Compute available memory (in GB)
        let available_memory = effective_memory_limit.saturating_sub(effective_memory_usage);
        let available_memory_gb = (available_memory as f32) / (1024.0 * 1024.0 * 1024.0);
        log::debug!("Available memory in container: {} GB", available_memory_gb);
        Ok(available_memory_gb)
    }
}

/// Recursively look up parent directories for a memory limit and usage
fn find_parent_memory_limit(cgroup_path: &str) -> Result<(i64, i64)> {
    let mut current_path = cgroup_path.to_string();

    // Walk up one level at a time until reaching the root
    while !current_path.is_empty() && current_path != "/" {
        // Get the parent directory path
        if let Some(last_slash) = current_path.rfind('/') {
            if last_slash == 0 {
                // Reached the root
                current_path = "".to_string();
            } else {
                current_path = current_path[..last_slash].to_string();
            }
        } else {
            break;
        }

        // Build paths for the memory-limit files at this level
        let memory_limit_path = format!(
            "/sys/fs/cgroup/memory{}/memory.limit_in_bytes",
            current_path
        );
        let memory_usage_path = format!(
            "/sys/fs/cgroup/memory{}/memory.usage_in_bytes",
            current_path
        );

        // Try to read the memory limit
        if let Ok(limit_content) = fs::read_to_string(&memory_limit_path) {
            let limit: i64 = limit_content.trim().parse().unwrap_or(i64::MAX);
            // Use it if we found a valid limit (not a sentinel huge value)
            if limit < i64::MAX - 1000000 {
                if let Ok(usage_content) = fs::read_to_string(&memory_usage_path) {
                    let usage: i64 = usage_content.trim().parse().unwrap_or(0);
                    return Ok((limit, usage));
                }
            }
        }
    }

    // No limit found anywhere; return a sentinel huge value to indicate "unlimited"
    Ok((i64::MAX, 0))
}
