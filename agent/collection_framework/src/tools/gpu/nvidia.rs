// src/tools/gpu/nvidia.rs - NVIDIA GPU Device Implementation

use super::common::{GpuDevice, GpuType};
use super::utils::{list_gpu_pids_with_proc_fd, list_gpu_pids_with_proc_maps};
use crate::error::ErrorCode;
use anyhow::Result;
use tracing as log;
use log::{debug, error};
use nvml_wrapper::NVML;
use std::collections::{HashMap, HashSet};
use std::process::Command;

pub struct NvidiaDevice;

impl NvidiaDevice {
    pub fn new() -> Self {
        Self
    }

    /// Scan using NVML library
    fn scan_with_nvml(&self) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let nvml = NVML::init().map_err(|e| {
            let err_msg = format!("Failed to initialize NVML: {}", e);
            error!("{}", err_msg);
            anyhow::anyhow!(err_msg)
        })?;

        let device_count = nvml.device_count().map_err(|e| {
            let err_msg = format!("Failed to get device count from NVML: {}", e);
            error!("{}", err_msg);
            anyhow::anyhow!(err_msg)
        })?;

        let mut all_pids = HashSet::new();
        let mut pid_to_gpu = HashMap::new();
        let mut gpu_to_pids = HashMap::new();

        for i in 0..device_count {
            match nvml.device_by_index(i) {
                Ok(device) => match device.running_compute_processes() {
                    Ok(processes) => {
                        let gpu_id = i as i32;
                        let mut pids_on_this_gpu = Vec::new();

                        for process in processes {
                            let pid = process.pid as i32;
                            all_pids.insert(pid);
                            pid_to_gpu.insert(pid, gpu_id);
                            pids_on_this_gpu.push(pid);
                        }

                        if !pids_on_this_gpu.is_empty() {
                            gpu_to_pids.insert(gpu_id, pids_on_this_gpu);
                        }
                    }
                    Err(e) => {
                        error!("Failed to get processes on GPU {}: {}", i, e);
                    }
                },
                Err(e) => {
                    error!("Failed to get device at index {}: {}", i, e);
                }
            }
        }

        let pids_vec: Vec<i32> = all_pids.into_iter().collect();
        Ok((pids_vec, pid_to_gpu, gpu_to_pids))
    }

    /// Scan using nsenter + nvidia-smi
    fn scan_with_nsenter(
        &self,
        pids: &[i32],
    ) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "nvidia-smi",
                "--query-compute-apps=pid,gpu_uuid",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to execute nvidia-smi with nsenter: {}", e))?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "nvidia-smi failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let mut detected_pids = Vec::new();
        let mut pid_to_gpu = HashMap::new();
        let mut gpu_to_pids: HashMap<i32, Vec<i32>> = HashMap::new();

        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            let parts: Vec<&str> = line.split(", ").collect();
            if parts.len() == 2 {
                if let (Ok(pid), Ok(gpu_uuid)) = (
                    parts[0].trim().parse::<i32>(),
                    Ok::<String, ()>(parts[1].trim().to_string()),
                ) {
                    if pids.is_empty() || pids.contains(&pid) {
                        detected_pids.push(pid);
                        if let Ok(gpu_id) = self.get_gpu_index_from_uuid(&gpu_uuid) {
                            pid_to_gpu.insert(pid, gpu_id);
                            gpu_to_pids.entry(gpu_id).or_insert_with(Vec::new).push(pid);
                        }
                    }
                }
            }
        }

        Ok((detected_pids, pid_to_gpu, gpu_to_pids))
    }

    /// Get GPU index from UUID
    fn get_gpu_index_from_uuid(&self, gpu_uuid: &str) -> Result<i32, String> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "nvidia-smi",
                "--query-gpu=uuid,index",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| format!("Failed to execute nvidia-smi to get GPU UUIDs: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "nvidia-smi failed to get GPU UUIDs: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            let parts: Vec<&str> = line.split(", ").collect();
            if parts.len() == 2 {
                if parts[0].trim() == gpu_uuid {
                    if let Ok(gpu_index) = parts[1].trim().parse::<i32>() {
                        return Ok(gpu_index);
                    }
                }
            }
        }

        Err(format!("GPU UUID {} not found in system", gpu_uuid))
    }

    /// Try fallback detection strategies in order
    fn try_fallback_strategies(&self, all_pids: &mut HashSet<i32>) {
        // Strategy 2: /dev + /proc/fd
        match list_gpu_pids_with_proc_fd("amperf", "nvidia") {
            Ok(pids) if !pids.is_empty() => {
                debug!("/dev+/proc/fd detection succeeded: {} PIDs", pids.len());
                all_pids.extend(&pids);
                return; // Success, no need to try next strategy
            }
            Ok(_) => {
                debug!("/dev+/proc/fd returned no PIDs -> trying Strategy 3");
            }
            Err(e) => {
                debug!("/dev+/proc/fd failed: {} -> trying Strategy 3", e);
            }
        }

        // Strategy 3: /proc/maps (last resort)
        match list_gpu_pids_with_proc_maps(&GpuType::Nvidia.devices()) {
            Ok(pids) if !pids.is_empty() => {
                debug!("/proc/maps detection succeeded: {} PIDs", pids.len());
                all_pids.extend(&pids);
            }
            Ok(_) => {
                debug!("/proc/maps returned no PIDs (all strategies exhausted)");
            }
            Err(e) => {
                debug!("/proc/maps failed: {} (all strategies exhausted)", e);
            }
        }
    }
}

impl GpuDevice for NvidiaDevice {
    fn gpu_type(&self) -> GpuType {
        GpuType::Nvidia
    }

    fn scan_processes(&self) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let mut all_pids: HashSet<i32> = HashSet::new();
        let mut pid_to_gpu: HashMap<i32, i32> = HashMap::new();
        let mut gpu_to_pids: HashMap<i32, Vec<i32>> = HashMap::new();

        // Cascading fallback strategy to avoid phantom PIDs
        // Try strategies in order: NVML -> /dev+/proc/fd -> /proc/maps
        // Only proceed to next strategy if current one returns empty results
        
        // Strategy 1: NVML (most reliable, provides PID→GPU mapping)
        let nvml_result = self.scan_with_nvml();
        match nvml_result {
            Ok((pids, p2g, g2p)) if !pids.is_empty() => {
                debug!("NVML detection succeeded: {} PIDs", pids.len());
                all_pids.extend(&pids);
                pid_to_gpu.extend(p2g);
                merge_gpu_to_pids(&mut gpu_to_pids, g2p);
            }
            Ok(_) => {
                debug!("NVML returned no PIDs -> trying Strategy 2");
                self.try_fallback_strategies(&mut all_pids);
            }
            Err(e) => {
                debug!("NVML failed: {} -> trying Strategy 2", e);
                self.try_fallback_strategies(&mut all_pids);
            }
        }

        // If NVML failed or returned no mapping, try nsenter+nvidia-smi to supplement mapping
        if pid_to_gpu.is_empty() && !all_pids.is_empty() {
            let pids_vec: Vec<i32> = all_pids.iter().copied().collect();
            if let Ok((_, p2g, g2p)) = self.scan_with_nsenter(&pids_vec) {
                debug!("nsenter+nvidia-smi supplement mapping succeeded: {} PIDs", p2g.len());
                pid_to_gpu.extend(p2g);
                merge_gpu_to_pids(&mut gpu_to_pids, g2p);
            }
        }

        let mut pids_vec: Vec<i32> = all_pids.into_iter().collect();
        pids_vec.sort_unstable();

        Ok((pids_vec, pid_to_gpu, gpu_to_pids))
    }

    fn get_memory_usage_by_pid(&self, pid: i32) -> Result<u64> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "nvidia-smi",
                "--query-compute-apps=pid,used_memory",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute nvidia-smi to get memory usage: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "nvidia-smi failed to get memory usage: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
            .into_error());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        for line in output_str.lines() {
            let parts: Vec<&str> = line.split(", ").collect();
            if parts.len() == 2 {
                if let (Ok(line_pid), Ok(memory_usage)) = (
                    parts[0].trim().parse::<i32>(),
                    parts[1].trim().parse::<u64>(),
                ) {
                    if line_pid == pid {
                        return Ok(memory_usage);
                    }
                }
            }
        }

        Err(
            ErrorCode::CommandFailed(Some(format!("PID {} not found or not using GPU", pid)))
                .into_error(),
        )
    }

    fn get_total_memory(&self, gpu_id: u32) -> Result<u64> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "nvidia-smi",
                "--query-gpu=memory.total",
                "--format=csv,noheader,nounits",
                "-i",
                &gpu_id.to_string(),
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute nvidia-smi to get total memory: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "nvidia-smi failed to get total memory: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
            .into_error());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let memory_total = output_str.lines().next().unwrap_or("").trim();

        if memory_total.is_empty() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "No memory information found for GPU {}",
                gpu_id
            )))
            .into_error());
        }

        let parts: Vec<&str> = memory_total.split_whitespace().collect();
        if parts.is_empty() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "Invalid memory format for GPU {}: {}",
                gpu_id, memory_total
            )))
            .into_error());
        }

        let memory_mb = parts[0].parse::<u64>().map_err(|e| {
            ErrorCode::CommandFailed(Some(format!(
                "Failed to parse memory value '{}': {}",
                parts[0], e
            )))
            .into_error()
        })?;

        Ok(memory_mb)
    }

    fn get_gpu_model(&self) -> Result<String> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "nvidia-smi",
                "--query-gpu=name",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute nvidia-smi to get GPU model: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "nvidia-smi failed to get GPU model: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
            .into_error());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let model_name = output_str.lines().next().unwrap_or("").trim();

        if model_name.is_empty() {
            return Err(
                ErrorCode::CommandFailed(Some("No GPU model found".to_string())).into_error(),
            );
        }

        Ok(model_name.to_string())
    }

    fn get_all_gpu_ids(&self) -> Result<Vec<i32>> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "nvidia-smi",
                "--query-gpu=index",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute nvidia-smi with nsenter: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "nvidia-smi failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
            .into_error());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut gpu_ids = Vec::new();

        for line in output_str.lines() {
            if let Ok(gpu_index) = line.trim().parse::<i32>() {
                gpu_ids.push(gpu_index);
            }
        }

        gpu_ids.push(-1);

        Ok(gpu_ids)
    }

    fn filter_gpu_pids(&self, target_pids: &[i32]) -> Result<Vec<i32>> {
        let (all_pids, pid_to_gpu_map, _) = self.scan_processes()?;
        
        let result: Vec<i32> = target_pids
            .iter()
            .filter(|&&pid| all_pids.contains(&pid) || pid_to_gpu_map.contains_key(&pid))
            .copied()
            .collect();
        
        Ok(result)
    }
}

/// Helper function to merge GPU to PIDs mapping
fn merge_gpu_to_pids(
    target: &mut HashMap<i32, Vec<i32>>,
    source: HashMap<i32, Vec<i32>>,
) {
    for (gpu_id, pids) in source {
        let entry = target.entry(gpu_id).or_insert_with(Vec::new);
        for pid in pids {
            if !entry.contains(&pid) {
                entry.push(pid);
            }
        }
    }
}
