// src/tools/gpu/common.rs - Common GPU Interface and Types

use anyhow::Result;
use std::collections::HashMap;

/// GPU vendor type enumeration
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub enum GpuType {
    Nvidia,
    AMD,
    PPU,
    Unknown,
}

impl GpuType {
    pub fn name(&self) -> &str {
        match self {
            GpuType::AMD => "AMD",
            GpuType::Nvidia => "Nvidia",
            GpuType::PPU => "PPU",
            GpuType::Unknown => "Unknown",
        }
    }

    pub fn devices(&self) -> Vec<String> {
        let device_map: HashMap<GpuType, Vec<String>> = HashMap::from([
            (
                GpuType::Nvidia,
                vec![
                    "/dev/nvidia".to_string(),
                    // "/dev/nvidiactl".to_string(),
                    // "/dev/nvidia-uvm".to_string(),
                ],
            ),
            (
                GpuType::AMD,
                vec!["/dev/dri/card".to_string(), "/dev/dri/renderD".to_string()],
            ),
            (GpuType::PPU, vec!["/dev/alixpu".to_string()]),
            (GpuType::Unknown, vec![]),
        ]);
        device_map[self].clone()
    }
}

/// Common trait for GPU operations
pub trait GpuDevice: Send + Sync {
    /// Get the GPU type this backend handles
    fn gpu_type(&self) -> GpuType;
    
    /// Scan for processes using GPU with mapping information
    /// Returns: (all_pids, pid_to_gpu_map, gpu_to_pids_map)
    fn scan_processes(&self) -> Result<(Vec<i32>, HashMap<i32, i32>, HashMap<i32, Vec<i32>>)>;
    
    /// Get GPU memory usage for a specific PID (in MiB)
    fn get_memory_usage_by_pid(&self, pid: i32) -> Result<u64>;
    
    /// Get total memory for a specific GPU (in MiB)
    fn get_total_memory(&self, gpu_id: u32) -> Result<u64>;
    
    /// Get GPU model name
    fn get_gpu_model(&self) -> Result<String>;
    
    /// Get all GPU IDs in the system
    fn get_all_gpu_ids(&self) -> Result<Vec<i32>>;
    
    /// Filter PIDs that are using this GPU type
    fn filter_gpu_pids(&self, target_pids: &[i32]) -> Result<Vec<i32>>;
}
