// src/tools/gpu_process.rs - GPU Process Management Utilities (Refactored)

use crate::error::ErrorCode;
use crate::tools::gpu::{self, AmdDevice, GpuDevice, GpuType, NvidiaDevice, PpuDevice};
use anyhow::Result;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::sync::RwLock;
use tracing as log;

// ============================================================================
// Global State: Unified GPU Process Cache
// ============================================================================
lazy_static! {
    /// Cache: All GPU process PIDs
    static ref GPU_PROCESS_CACHE: RwLock<Option<Vec<i32>>> = RwLock::new(None);
    
    /// Cache: PID -> GPU ID mapping
    static ref PID_TO_GPU_ID_MAP: RwLock<HashMap<i32, i32>> = RwLock::new(HashMap::new());
    
    /// Cache: GPU ID -> PID list mapping
    static ref GPU_ID_TO_PIDS_MAP: RwLock<HashMap<i32, Vec<i32>>> = RwLock::new(HashMap::new());
    
    /// Cache: Detected GPU type
    static ref DETECTED_GPU_TYPE: RwLock<Option<GpuType>> = RwLock::new(None);
    
    /// Cache: GPU backend instance
    static ref GPU_BACKEND: RwLock<Option<Box<dyn GpuDevice>>> = RwLock::new(None);
}

/// Detect GPU type in system
pub fn detect_gpu_type() -> Option<GpuType> {
    let vendor_map = [
        ("0x10de", GpuType::Nvidia),
        ("0x1ded", GpuType::PPU),
        ("0x1002", GpuType::AMD),
    ];

    let devices_path: &str = "/sys/bus/pci/devices/";

    // Try to read PCI device directory
    if let Ok(entries) = fs::read_dir(devices_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                let device_name = entry.file_name();
                let device_name_str = device_name.to_string_lossy();

                // Check device class
                let class_path = format!("{}{}/class", devices_path, device_name_str);
                if let Ok(class_content) = fs::read_to_string(class_path) {
                    if class_content.len() >= 4 {
                        let class_prefix = &class_content[..4];

                        // Check if it's a GPU or accelerator device
                        if class_prefix == "0x03" || class_prefix == "0x12" {
                            // Check vendor
                            let vendor_path =
                                format!("{}{}/vendor", devices_path, device_name_str);
                            if let Ok(vendor_content) = fs::read_to_string(vendor_path) {
                                if vendor_content.len() >= 6 {
                                    let vendor_prefix = &vendor_content[..6];

                                    // Find matching vendor in map
                                    for (vendor_id, vendor_name) in &vendor_map {
                                        if vendor_prefix == *vendor_id {
                                            return Some(vendor_name.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: check smi commands
    if Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some(GpuType::Nvidia);
    }
    if Command::new("ppu-smi")
        .arg("-L")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Some(GpuType::PPU);
    }

    None
}

// ============================================================================
// Public API: Unified GPU Process Detection with Caching
// ============================================================================

/// Create GPU backend based on GPU type
fn create_backend(gpu_type: &GpuType) -> Box<dyn GpuDevice> {
    match gpu_type {
        GpuType::Nvidia => Box::new(NvidiaDevice::new()),
        GpuType::AMD => Box::new(AmdDevice::new()),
        GpuType::PPU => Box::new(PpuDevice::new()),
        GpuType::Unknown => Box::new(NvidiaDevice::new()), // Default to Nvidia
    }
}

/// Refresh GPU process cache
pub fn refresh_gpu_process_cache() -> Result<()> {
    let gpu_type = detect_gpu_type().unwrap_or(GpuType::Unknown);
    
    {
        let mut cached_type = DETECTED_GPU_TYPE.write().unwrap();
        *cached_type = Some(gpu_type.clone());
    }
    
    log::debug!("Refreshing GPU process cache for GPU type: {:?}", gpu_type);
    
    let backend = create_backend(&gpu_type);
    let (all_pids, pid_to_gpu, gpu_to_pids) = backend.scan_processes()?;
    
    {
        let mut cache = GPU_PROCESS_CACHE.write().unwrap();
        *cache = Some(all_pids);
    }
    
    {
        let mut map = PID_TO_GPU_ID_MAP.write().unwrap();
        *map = pid_to_gpu;
    }
    
    {
        let mut map = GPU_ID_TO_PIDS_MAP.write().unwrap();
        *map = gpu_to_pids;
    }
    
    {
        let mut backend_cache = GPU_BACKEND.write().unwrap();
        *backend_cache = Some(backend);
    }
    
    log::debug!("GPU process cache refreshed successfully  GPU_PROCESS_CACHE: {:#?}, PID_TO_GPU_ID_MAP:{:#?}, GPU_ID_TO_PIDS_MAP:{:#?}, DETECTED_GPU_TYPE:{:#?}", GPU_PROCESS_CACHE.read().unwrap(), PID_TO_GPU_ID_MAP.read().unwrap(), GPU_ID_TO_PIDS_MAP.read().unwrap(), DETECTED_GPU_TYPE.read().unwrap());
    Ok(())
}

/// List processes using GPU
pub fn list_gpu_processes() -> Result<Vec<i32>> {
    {
        let cache = GPU_PROCESS_CACHE.read().unwrap();
        if let Some(ref pids) = *cache {
            log::debug!("Returning cached GPU processes: {} PIDs", pids.len());
            return Ok(pids.clone());
        }
    }
    
    log::debug!("GPU process cache empty, refreshing...");
    refresh_gpu_process_cache()?;
    
    let cache = GPU_PROCESS_CACHE.read().unwrap();
    Ok(cache.as_ref().unwrap().clone())
}

/// List processes using specified GPU type
pub fn list_gpu_processes_for(gpu_type: GpuType) -> Result<Vec<i32>> {
    let backend = create_backend(&gpu_type);
    let (all_pids, _, _) = backend.scan_processes()?;
    Ok(all_pids)
}

// ============================================================================
// Legacy API Compatibility Layer
// ============================================================================

/// Get PIDs which are using GPU
pub fn get_pids_on_gpu() -> Vec<i32> {
    let vec = GPU_PROCESS_CACHE.read().unwrap().clone().unwrap_or(vec![]);
    vec.clone()
}

/// Get GPU ID by PID
pub fn get_gpu_id_by_pid(pid: i32) -> Option<i32> {
    log::debug!("Get PID_TO_GPU_ID_MAP:{:?}", PID_TO_GPU_ID_MAP.read().unwrap());
    let map = PID_TO_GPU_ID_MAP.read().unwrap();
    map.get(&pid).copied()
}

/// Convert PID to GPU ID (Nvidia)
pub fn convert_pid_to_gpu_id_nvidia(pids: &[i32]) -> Result<Vec<i32>, String> {
    with_backend(GpuType::Nvidia, |backend| {
        backend.scan_processes().map(|(_, pid_to_gpu, _)| {
            let mut gpu_ids: Vec<i32> = pids
                .iter()
                .filter_map(|&pid| pid_to_gpu.get(&pid).copied())
                .collect();
            gpu_ids.sort_unstable();
            gpu_ids.dedup();
            gpu_ids.push(-1); // Sentinel value
            gpu_ids
        })
    })
    .map_err(|e| format!("Failed to scan GPU processes: {}", e))
}

/// Get PIDs on Nvidia GPUs
pub fn get_pids_on_nvidia(target_pids: &[i32]) -> Vec<i32> {
    get_pids_on_gpu()
}

/// Get all GPU IDs
pub fn get_all_gpuids() -> Result<Vec<i32>> {
    with_backend(GpuType::Nvidia, |backend| backend.get_all_gpu_ids())
}

/// Get PIDs on AMD GPUs
pub fn get_pids_on_amd(target_pids: &[i32]) -> Result<Vec<i32>> {
    with_backend(GpuType::AMD, |backend| backend.filter_gpu_pids(target_pids))
}

/// Get PIDs on PPU GPUs
pub fn get_pids_on_ppu(pids: &[i32]) -> Result<Vec<i32>, String> {
    with_backend(GpuType::PPU, |backend| backend.filter_gpu_pids(pids))
        .map_err(|e| e.to_string())
}

/// Execute operation using cached backend if available and matches GPU type
/// Otherwise create a new backend instance
fn with_backend<F, T>(gpu_type: GpuType, f: F) -> Result<T>
where
    F: FnOnce(&dyn GpuDevice) -> Result<T>,
{
    // Check if cached backend matches the requested GPU type
    {
        let backend_cache = GPU_BACKEND.read().unwrap();
        if let Some(ref backend) = *backend_cache {
            if backend.gpu_type() == gpu_type {
                log::debug!("Using cached backend for GPU type: {:?}", gpu_type);
                return f(backend.as_ref());
            }
        }
    }
    
    // Create new backend if no cache or type mismatch
    log::debug!("Creating new backend for GPU type: {:?}", gpu_type);
    let backend = create_backend(&gpu_type);
    f(backend.as_ref())
}

/// Get Nvidia GPU model
pub fn get_nvidia_gpu_model() -> Result<String> {
    with_backend(GpuType::Nvidia, |backend| backend.get_gpu_model())
}

/// Get PPU GPU model
pub fn get_ppu_gpu_model() -> Result<String> {
    with_backend(GpuType::PPU, |backend| backend.get_gpu_model())
}

/// Get AMD GPU models
pub fn get_amd_gpu_models() -> Result<HashMap<u32, String>> {
    // Note: AMD backend might need vendor-specific method
    let backend = AmdDevice::new();
    backend.get_all_gpu_models()
}

/// Get GPU memory usage by PID
pub fn get_gpu_memory_usage_by_pid(pid: i32) -> Result<u64> {
    let gpu_type = detect_gpu_type().ok_or_else(|| {
        ErrorCode::CommandFailed(Some("Failed to detect GPU type".to_string())).into_error()
    })?;

    let backend = create_backend(&gpu_type);
    backend.get_memory_usage_by_pid(pid)
}

/// Get Nvidia GPU total memory
pub fn get_nvidia_gpu_memory_total(gpu_id: u32) -> Result<u64> {
    with_backend(GpuType::Nvidia, |backend| backend.get_total_memory(gpu_id))
}

/// Get PPU GPU total memory
pub fn get_ppu_gpu_memory_total(gpu_id: u32) -> Result<u64> {
    with_backend(GpuType::PPU, |backend| backend.get_total_memory(gpu_id))
}

/// List GPU PIDs with /proc/fd (re-export from utils)
pub fn list_gpu_pids_with_proc_fd(besides_key: &str, devices_key: &str) -> Result<Vec<i32>> {
    gpu::utils::list_gpu_pids_with_proc_fd(besides_key, devices_key)
}
