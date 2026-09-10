// src/tools/gpu/utils.rs - Common Utility Functions

use anyhow::Result;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use tracing as log;

const DEV_PATH: &str = "/dev";
const PROC_PATH: &str = "/proc";

/// Get all device character files matching the key
fn get_all_device_character(dir_path: &str, dev_key: &str) -> Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    for entry in fs::read_dir(dir_path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if name_str.contains(dev_key) {
            results.push(entry.path());
        }
    }

    if results.is_empty() {
        return Err(anyhow::anyhow!("no device character found for key: {}", dev_key));
    }
    Ok(results)
}

/// Get device and inode information
fn get_device_and_inode(path: &PathBuf) -> Result<(u64, u64)> {
    let meta = fs::metadata(path)?;
    Ok((meta.dev(), meta.ino()))
}

/// Check if device is in fd directory
fn is_device_in_fd_dir(
    pid: i32,
    dev: u64,
    ino: u64,
    dev_path: &PathBuf,
    besides_key: &str,
) -> bool {
    let pid_str = pid.to_string();
    let cmdline_path = format!("{}/{}/cmdline", PROC_PATH, pid_str);
    let cmdline_data = fs::read(cmdline_path).unwrap_or_default();

    let fd_dir = format!("{}/{}/fd", PROC_PATH, pid_str);
    let entries = match fs::read_dir(&fd_dir) {
        Ok(e) => e,
        Err(_) => return false,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let link_path = entry.path();

        // Try read_link first
        if let Ok(target) = fs::read_link(&link_path) {
            if target == *dev_path {
                // If cmdline contains besides_key, skip
                if !besides_key.is_empty()
                    && String::from_utf8_lossy(&cmdline_data).contains(besides_key)
                {
                    continue;
                }
                return true;
            }
        }

        // Compare dev/ino via stat
        if let Ok(meta) = fs::metadata(&link_path) {
            if meta.dev() == dev && meta.ino() == ino {
                return true;
            }
        }
    }
    false
}

/// List GPU PIDs using /proc/fd
pub fn list_gpu_pids_with_proc_fd(besides_key: &str, devices_key: &str) -> Result<Vec<i32>> {
    // 1. Get GPU device character files
    let gpu_device_characters = get_all_device_character(DEV_PATH, devices_key)?;

    // 2. Enumerate all process directories under /proc
    let proc_entries = fs::read_dir(PROC_PATH)?;
    let mut proc_pids: Vec<i32> = Vec::new();
    for entry in proc_entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name_str = match file_name.to_str() {
            Some(s) => s,
            None => continue,
        };
        // Only care about numeric directories
        if let Ok(pid) = file_name_str.parse::<i32>() {
            proc_pids.push(pid);
        }
    }

    let mut gpu_process_ids: Vec<i32> = Vec::new();

    // 3. For each GPU device file, check which PIDs are using it
    for gpu_device_character in gpu_device_characters {
        let (dev, ino) = match get_device_and_inode(&gpu_device_character) {
            Ok(di) => di,
            Err(e) => {
                log::error!(
                    "Failed to get device/inode for {:?}: {}",
                    gpu_device_character,
                    e
                );
                continue;
            }
        };

        for pid in &proc_pids {
            if is_device_in_fd_dir(*pid, dev, ino, &gpu_device_character, besides_key) {
                gpu_process_ids.push(*pid);
            }
        }
    }

    Ok(gpu_process_ids)
}

/// List GPU PIDs using /proc/maps
pub fn list_gpu_pids_with_proc_maps(devices: &[String]) -> Result<Vec<i32>> {
    let mut pids: Vec<i32> = Vec::new();
    let entries = fs::read_dir("/proc")?;

    for entry in entries {
        let entry = entry?;
        let pid_str = entry.file_name().to_string_lossy().to_string();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let pid: i32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let proc_root = entry.path();
        let mut is_gpu_process = false;

        if let Ok(maps) = fs::read_to_string(proc_root.join("maps")) {
            for line in maps.lines() {
                if devices.iter().any(|d| line.contains(d)) {
                    is_gpu_process = true;
                    break;
                }
            }
        }

        if is_gpu_process {
            pids.push(pid);
        }
    }

    Ok(pids)
}
