// src/tools/gpu/amd.rs - AMD GPU Device Implementation

use super::common::{GpuDevice, GpuType};
use super::utils::list_gpu_pids_with_proc_maps;
use crate::error::ErrorCode;
use anyhow::Result;
use tracing as log;
use log::debug;
use std::collections::{HashMap, HashSet};
use std::process::Command;

pub struct AmdDevice;

impl AmdDevice {
    pub fn new() -> Self {
        Self
    }

    /// Scan using rocm-smi
    fn scan_with_rocm_smi(
        &self,
        target_pids: &[i32],
    ) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "rocm-smi",
                "--showpidgpus",
            ])
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to execute rocm-smi: {}", e))?;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "rocm-smi failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = output_str.lines().collect();

        let mut detected_pids = Vec::new();
        let mut pid_to_gpu = HashMap::new();

        for i in 0..lines.len() {
            let line = lines[i];
            if line.contains("PID") && line.contains("is using") && !line.contains("is using 0 DRM device") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(pid) = parts[1].parse::<i32>() {
                        if target_pids.is_empty() || target_pids.contains(&pid) {
                            detected_pids.push(pid);

                            // Try to parse GPU ID from next line
                            if i + 1 < lines.len() {
                                let gpu_line = lines[i + 1].trim();
                                let gpu_ids: Vec<&str> = gpu_line.split_whitespace().collect();
                                if let Some(first_gpu_id) = gpu_ids.first() {
                                    if let Ok(gpu_id) = first_gpu_id.parse::<i32>() {
                                        pid_to_gpu.insert(pid, gpu_id);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Build reverse mapping
        let mut gpu_to_pids: HashMap<i32, Vec<i32>> = HashMap::new();
        for (pid, gpu_id) in &pid_to_gpu {
            gpu_to_pids.entry(*gpu_id).or_insert_with(Vec::new).push(*pid);
        }

        Ok((detected_pids, pid_to_gpu, gpu_to_pids))
    }
}

impl GpuDevice for AmdDevice {
    fn gpu_type(&self) -> GpuType {
        GpuType::AMD
    }

    fn scan_processes(&self) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)> {
        let mut all_pids: HashSet<i32> = HashSet::new();
        let mut pid_to_gpu: HashMap<i32, i32> = HashMap::new();

        // Strategy 1: rocm-smi (preferred, provides PID->GPU mapping)
        match self.scan_with_rocm_smi(&[]) {
            Ok((pids, p2g, _)) if !pids.is_empty() => {
                debug!("rocm-smi detection: {} PIDs", pids.len());
                all_pids.extend(&pids);
                pid_to_gpu.extend(p2g);
            }
            Ok(_) => {
                debug!("rocm-smi returned no PIDs -> falling back to /proc/maps");
                // Strategy 2: /proc/maps (fallback when rocm-smi has no data)
                if let Ok(pids) = list_gpu_pids_with_proc_maps(&GpuType::AMD.devices()) {
                    debug!("AMD maps detection: {} PIDs", pids.len());
                    all_pids.extend(&pids);
                }
            }
            Err(e) => {
                debug!("rocm-smi failed: {} -> falling back to /proc/maps", e);
                // Strategy 2: /proc/maps (fallback)
                if let Ok(pids) = list_gpu_pids_with_proc_maps(&GpuType::AMD.devices()) {
                    debug!("AMD maps detection: {} PIDs", pids.len());
                    all_pids.extend(&pids);
                }
            }
        }

        // Build GPU to PIDs mapping
        let mut gpu_to_pids: HashMap<i32, Vec<i32>> = HashMap::new();
        for (pid, gpu_id) in &pid_to_gpu {
            gpu_to_pids.entry(*gpu_id).or_insert_with(Vec::new).push(*pid);
        }

        let mut pids_vec: Vec<i32> = all_pids.into_iter().collect();
        pids_vec.sort_unstable();

        Ok((pids_vec, pid_to_gpu, gpu_to_pids))
    }

    fn get_memory_usage_by_pid(&self, _pid: i32) -> Result<u64> {
        // TODO: rocm-smi output format is complex, placeholder implementation
        Ok(0)
    }

    fn get_total_memory(&self, _gpu_id: u32) -> Result<u64> {
        // TODO: Not commonly used for AMD, placeholder implementation
        Ok(0)
    }

    fn get_gpu_model(&self) -> Result<String> {
        let models = self.get_all_gpu_models()?;
        
        // Return the first model if available
        if let Some((_, model)) = models.iter().next() {
            Ok(model.clone())
        } else {
            Err(ErrorCode::CommandFailed(Some("No AMD GPU model found".to_string())).into_error())
        }
    }

    fn get_all_gpu_ids(&self) -> Result<Vec<i32>> {
        let models = self.get_all_gpu_models()?;
        let mut gpu_ids: Vec<i32> = models.keys().map(|&k| k as i32).collect();
        gpu_ids.sort_unstable();
        gpu_ids.push(-1);
        Ok(gpu_ids)
    }

    fn filter_gpu_pids(&self, target_pids: &[i32]) -> Result<Vec<i32>> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "rocm-smi",
                "--showpidgpus",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute rocm-smi with nsenter: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "rocm-smi failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
            .into_error());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut amd_pids = HashSet::new();
        let lines: Vec<&str> = output_str.lines().collect();

        for i in 0..lines.len() {
            let line = lines[i];
            if line.contains("PID") && line.contains("is using") {
                if !line.contains("is using 0 DRM device") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(pid) = parts[1].parse::<i32>() {
                            amd_pids.insert(pid);
                        }
                    }
                }
            }
        }

        let mut result = Vec::new();
        for &pid in target_pids {
            if amd_pids.contains(&pid) {
                result.push(pid);
            }
        }

        Ok(result)
    }
}

impl AmdDevice {
    /// Get all AMD GPU models
    pub fn get_all_gpu_models(&self) -> Result<HashMap<u32, String>> {
        let output = Command::new("nsenter")
            .args([
                "-t", "1", "-m", "-u", "-i", "-n", "-p",
                "rocm-smi",
                "--showproductname",
            ])
            .output()
            .map_err(|e| {
                ErrorCode::CommandFailed(Some(format!(
                    "Failed to execute rocm-smi to get GPU models: {}",
                    e
                )))
                .into_error()
            })?;

        if !output.status.success() {
            return Err(ErrorCode::CommandFailed(Some(format!(
                "rocm-smi failed to get GPU models: {}",
                String::from_utf8_lossy(&output.stderr)
            )))
            .into_error());
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        let mut gpu_models = HashMap::new();

        for line in output_str.lines() {
            if line.contains("Card Series:") {
                let parts: Vec<&str> = line.split(":").collect();
                if parts.len() >= 3 {
                    let gpu_part = parts[0].trim();
                    let model_part = parts[2].trim();

                    if let Some(start) = gpu_part.find("[") {
                        if let Some(end) = gpu_part.find("]") {
                            if let Ok(gpu_index) = gpu_part[start + 1..end].parse::<u32>() {
                                gpu_models.insert(gpu_index, model_part.to_string());
                            }
                        }
                    }
                }
            }
        }

        Ok(gpu_models)
    }
}
