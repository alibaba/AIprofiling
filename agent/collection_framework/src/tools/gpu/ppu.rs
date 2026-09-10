// src/tools/gpu/ppu.rs - PPU GPU Device Implementation

use super::common::{GpuDevice, GpuType};
use super::utils::{list_gpu_pids_with_proc_fd, list_gpu_pids_with_proc_maps};
use crate::error::ErrorCode;
use anyhow::Result;
use tracing as log;
use log::{debug, error};
use std::collections::{HashMap, HashSet};
use std::process::Command;

pub struct PpuDevice;

impl PpuDevice {
    pub fn new() -> Self {
        Self
    }

    /// Scan using ppu-smi
    fn scan_with_ppu_smi(
        &self,
        pids: &[i32],
    ) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let output_res = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "ppu-smi",
                "--query-compute-apps=pid,gpu_uuid",
                "--format=csv,noheader,nounits",
            ])
            .output();

        let output = match output_res {
            Ok(out) if out.status.success() => Ok(out),
            _ => Command::new("ppu-smi")
                .args([
                    "--query-compute-apps=pid,gpu_uuid",
                    "--format=csv,noheader,nounits",
                ])
                .output(),
        };

        let mut detected_pids = Vec::new();
        let mut pid_to_gpu = HashMap::new();
        let mut gpu_to_pids: HashMap<i32, Vec<i32>> = HashMap::new();

        if let Ok(output) = output {
            if output.status.success() {
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
            }
        }

        // If no mapping, assign default GPU ID 0
        if pid_to_gpu.is_empty() && !pids.is_empty() {
            error!("Failed to get PID to GPU mapping from ppu-smi, using default GPU ID 0");
            for &pid in pids {
                pid_to_gpu.insert(pid, 0);
                gpu_to_pids.entry(0).or_insert_with(Vec::new).push(pid);
            }
            detected_pids.extend_from_slice(pids);
        }

        Ok((detected_pids, pid_to_gpu, gpu_to_pids))
    }

    /// Get GPU index from UUID
    fn get_gpu_index_from_uuid(&self, gpu_uuid: &str) -> Result<i32, String> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "ppu-smi",
                "--query-gpu=uuid,index",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| format!("Failed to execute ppu-smi to get GPU UUIDs: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "ppu-smi failed to get GPU UUIDs: {}",
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

    /// Try fallback detection strategies in order for PPU
    fn try_fallback_strategies(&self, all_pids: &mut HashSet<i32>) {
        // Strategy 2: /dev + /proc/fd
        match list_gpu_pids_with_proc_fd("amperf", "alixpu_ppu") {
            Ok(pids) if !pids.is_empty() => {
                debug!("PPU dev/fd detection: {} PIDs", pids.len());
                all_pids.extend(&pids);
                return; // success, don't try next strategy
            }
            Ok(_) => {
                debug!("PPU dev/fd returned no PIDs -> trying /proc/maps");
            }
            Err(e) => {
                debug!("PPU dev/fd failed: {} -> trying /proc/maps", e);
            }
        }

        // Strategy 3: /proc/maps (last resort)
        match list_gpu_pids_with_proc_maps(&GpuType::PPU.devices()) {
            Ok(pids) if !pids.is_empty() => {
                debug!("PPU maps detection: {} PIDs", pids.len());
                all_pids.extend(&pids);
            }
            Ok(_) => {
                debug!("PPU maps returned no PIDs (all strategies exhausted)");
            }
            Err(e) => {
                debug!("PPU maps failed: {} (all strategies exhausted)", e);
            }
        }
    }
}

impl GpuDevice for PpuDevice {
    fn gpu_type(&self) -> GpuType {
        GpuType::PPU
    }

    fn scan_processes(&self) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let mut all_pids: HashSet<i32> = HashSet::new();
        let mut pid_to_gpu: HashMap<i32, i32> = HashMap::new();
        let mut gpu_to_pids: HashMap<i32, Vec<i32>> = HashMap::new();
    
        // Strategy 1: ppu-smi (preferred, provides PID->GPU mapping)
        let ppu_result = self.scan_with_ppu_smi(&[]);
        match ppu_result {
            Ok((pids, p2g, g2p)) if !pids.is_empty() => {
                debug!("ppu-smi detection: {} PIDs", pids.len());
                all_pids.extend(&pids);
                pid_to_gpu.extend(p2g);
                merge_gpu_to_pids(&mut gpu_to_pids, g2p);
            }
            Ok(_) => {
                debug!("ppu-smi returned no PIDs -> falling back to /dev+fd then /proc/maps");
                self.try_fallback_strategies(&mut all_pids);
            }
            Err(e) => {
                debug!("ppu-smi failed: {} -> falling back to /dev+fd then /proc/maps", e);
                self.try_fallback_strategies(&mut all_pids);
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
                "ppu-smi",
                "--query-compute-apps=pid,used_memory",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute ppu-smi to get memory usage: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "ppu-smi failed to get memory usage: {}",
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
                "ppu-smi",
                "--query-gpu=memory.total",
                "--format=csv,noheader,nounits",
                "-i",
                &gpu_id.to_string(),
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute ppu-smi to get total memory: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "ppu-smi failed to get total memory: {}",
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
                "ppu-smi",
                "--query-gpu=name",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute ppu-smi to get GPU model: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "ppu-smi failed to get GPU model: {}",
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
                "ppu-smi",
                "--query-gpu=index",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute ppu-smi with nsenter: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "ppu-smi failed: {}",
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
        let (_, pid_to_gpu, _) = self.scan_with_ppu_smi(target_pids)?;
        
        let result: Vec<i32> = target_pids
            .iter()
            .filter(|&&pid| pid_to_gpu.contains_key(&pid))
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
