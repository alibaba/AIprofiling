// src/detectors/detectors.rs
use crate::collector::collector_name::CollectorName;
use crate::collector::indicator_name::IndicatorName;
use crate::command::ProfileArgs;
use crate::config::DependencyConfig;
use crate::error::ErrorCode;
use crate::meta::Meta;
use crate::tools::gpu_process::{
    detect_gpu_type, get_gpu_id_by_pid, get_gpu_memory_usage_by_pid, get_nvidia_gpu_memory_total,
    get_nvidia_gpu_model, get_pids_on_nvidia, get_pids_on_ppu, get_ppu_gpu_memory_total,
    get_ppu_gpu_model, list_gpu_processes, list_gpu_processes_for,
};
use crate::tools::gpu::GpuType;
use crate::tools::utils::get_python_executable;
use anyhow::Result;
use log::debug;
use std::collections::{HashMap, HashSet};
use std::process::Command;
use tracing as log;

pub fn detect_nvidia_or_ppu(
    config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    if let Some(gpu_type) = detect_gpu_type() {
        match gpu_type {
            GpuType::Nvidia => {
                return detect_nvidia(config, args, meta);
            }
            GpuType::PPU => {
                return detect_ppu(config, args, meta);
            }
            _ => {
                return Err(ErrorCode::DetectEnvError(Some(
                    "Non-Nvidia or PPU environment".to_string(),
                ))
                .into_error());
            }
        }
    } else {
        for pid in &args.pids {
            let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
            meta_entry.registor_indicator(
                &IndicatorName::GPU,
                &CollectorName::Pyki,
                "Failed",
                "Failed to detect GPU type",
            );
        }
        return Ok(vec![]);
    }
}

// If the container mounts pciutils it can be installed at image build time;
// otherwise we detect nvidia-smi or ppu-smi inside the container.
// Return the list of PIDs that satisfy the condition.
pub fn detect_nvidia(
    _config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    if let Some(gpu_type) = detect_gpu_type() {
        match gpu_type {
            GpuType::Nvidia => {
                let pids = get_pids_on_nvidia(&args.pids);

                let gpu_model = get_nvidia_gpu_model().unwrap_or_else(|_| "nvidia".to_string());

                for pid in &args.pids {
                    let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
                    let gpu_id = get_gpu_id_by_pid(*pid).unwrap_or(0);
                    let used_gpu_memory = get_gpu_memory_usage_by_pid(*pid).unwrap_or(0);
                    let total_gpu_memory = get_nvidia_gpu_memory_total(gpu_id as u32).unwrap_or(0);
                    meta_entry.update_or_register_device_properties(
                        &gpu_model,
                        gpu_id,
                        total_gpu_memory,
                        used_gpu_memory,
                    );

                    if !pids.contains(pid) {
                        meta_entry.registor_indicator(
                            &IndicatorName::GPU,
                            &CollectorName::Pyki,
                            "Failed",
                            "Process is not running on NVIDIA GPU",
                        );
                    }
                }
                return Ok(pids);
            }
            _ => {
                let error_msg = format!("Non-Nvidia environment (detected {:?})", gpu_type);
                for pid in &args.pids {
                    let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
                    meta_entry.registor_indicator(
                        &IndicatorName::GPU,
                        &CollectorName::Pyki,
                        "Failed",
                        &error_msg,
                    );
                }
                return Err(ErrorCode::DetectEnvError(Some(error_msg)).into_error());
            }
        };
    }

    if let Some(gpu_type) = detect_gpu_type() {
        if matches!(gpu_type, GpuType::PPU | GpuType::AMD) {
            debug!("Skipping nvidia detection in {:?} environment", gpu_type);
            return Ok(vec![]);
        }
    }

    let device_process = list_gpu_processes_for(GpuType::Nvidia).unwrap_or_else(|e| {
        debug!(
            "Fallback list_gpu_processes_for(Nvidia) failed: {}",
            e
        );
        Vec::new()
    });
    let set1: HashSet<i32> = device_process.into_iter().collect();
    let set2: HashSet<i32> = args.pids.clone().into_iter().collect();
    let intersection: Vec<i32> = set1.intersection(&set2).copied().collect();
    if !intersection.is_empty() {
        log::warn!(
            "Some processes are running on GPU but not detected by NVML: {:?}",
            intersection
        );
        return Ok(intersection);
    }

    for pid in &args.pids {
        let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
        meta_entry.registor_indicator(
            &IndicatorName::GPU,
            &CollectorName::Pyki,
            "Failed",
            "No NVIDIA GPU detected",
        );
    }
    Ok(vec![])
}

pub fn detect_ppu(
    _config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    if let Some(gpu_type) = detect_gpu_type() {
        match gpu_type {
            GpuType::PPU => {
                let all_gpu_pids = match list_gpu_processes_for(GpuType::PPU) {
                    Ok(pids) => pids,
                    Err(e) => {
                        return Err(
                            ErrorCode::DetectEnvError(None)
                                .with_details(format!("Failed to discover GPU processes: {}", e))
                                .into_error(),
                        );
                    }
                };

                let pids_on_gpu: Vec<i32> = args
                    .pids
                    .iter()
                    .filter(|pid| all_gpu_pids.contains(pid))
                    .copied()
                    .collect();

                let result = get_pids_on_ppu(&pids_on_gpu);
                match result {
                    Ok(_gpu_ids) => {
                        let gpu_model = get_ppu_gpu_model().unwrap_or_else(|_| "ppu".to_string());

                        for pid in &args.pids {
                            let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
                            let gpu_id = get_gpu_id_by_pid(*pid).unwrap_or(0);
                            let used_gpu_memory = get_gpu_memory_usage_by_pid(*pid).unwrap_or(0);
                            let total_gpu_memory =
                                get_ppu_gpu_memory_total(gpu_id as u32).unwrap_or(0);
                            meta_entry.update_or_register_device_properties(
                                &gpu_model,
                                gpu_id,
                                total_gpu_memory,
                                used_gpu_memory,
                            );

                            if !pids_on_gpu.contains(pid) {
                                meta_entry.registor_indicator(
                                    &IndicatorName::GPU,
                                    &CollectorName::Pyki,
                                    "Failed",
                                    "Process is not running on NVIDIA GPU",
                                );
                            }
                        }
                        return Ok(pids_on_gpu);
                    }
                    Err(e) => {
                        return Err(ErrorCode::DetectEnvError(None)
                            .with_details(format!("Failed to get PIDs on PPU: {}", e))
                            .into_error());
                    }
                }
            }
            _ => {
                return Err(
                    ErrorCode::DetectEnvError(Some("Non-PPU environment".to_string())).into_error(),
                );
            }
        };
    }

    Ok(vec![])
}

/// Check whether the Python version is within the required range.
// Returns the list of PIDs that satisfy the condition.
pub fn detect_python_support(
    config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    let mut valid_pids = Vec::new();

    for pid in &args.pids {
        let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);

        let pyexec_output_str = get_python_executable(*pid).unwrap_or_default();

        if pyexec_output_str.is_empty() {
            debug!("Failed to get executable path for PID {}", pid);
            continue;
        }

        let mut success = false;
        let mut error_message = String::new();

        let nsenter_output = Command::new("nsenter")
            .args(&[
                "--target",
                &pid.to_string(),
                "--mount",
                "--uts",
                "--ipc",
                "--net",
                "--pid",
                &pyexec_output_str,
                "--version",
            ])
            .output();

        let version_output = match nsenter_output {
            Ok(out) if out.status.success() => Ok(out),
            _ => Command::new(&pyexec_output_str).arg("--version").output(),
        };

        match version_output {
            Ok(v_out) if v_out.status.success() => {
                let stdout = String::from_utf8_lossy(&v_out.stdout);
                let stderr = String::from_utf8_lossy(&v_out.stderr);
                let combined = format!("{}{}", stdout, stderr);

                if let Some(version) = combined
                    .split_whitespace()
                    .find(|s| s.chars().next().map_or(false, |c| c.is_ascii_digit()))
                {
                    if check_version_in_range(version, config).is_ok() {
                        valid_pids.push(*pid);
                        success = true;
                    } else {
                        error_message = format!("Python version {} not in range", version);
                    }
                } else {
                    error_message = "Failed to parse Python version string".to_string();
                }
            }
            Ok(v_out) => {
                error_message = format!(
                    "Python --version failed: {}",
                    String::from_utf8_lossy(&v_out.stderr)
                );
            }
            Err(e) => {
                error_message = format!("Failed to execute Python --version: {}", e);
            }
        }

        if !success {
            let failed_indicators = vec![
                (
                    IndicatorName::GPU,
                    CollectorName::Pyki,
                    error_message.clone(),
                ),
                (IndicatorName::PyStack, CollectorName::Pyki, error_message),
            ];

            for (indicator, collector, reason) in failed_indicators {
                meta_entry.registor_indicator(&indicator, &collector, "Failed", &reason);
            }
        }
    }

    Ok(valid_pids)
}

/// Check whether the PyTorch version is within the required range.
// Returns the list of PIDs that satisfy the condition.
pub fn detect_torch_support(
    config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    let mut valid_pids = Vec::new();

    for pid in &args.pids {
        let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);

        let pyexec_output_str = get_python_executable(*pid).unwrap_or_default();

        let mut success = false;
        let mut error_message = String::new();

        if !pyexec_output_str.is_empty() {
            let nsenter_pip_output = Command::new("nsenter")
                .args(&[
                    "--target",
                    &pid.to_string(),
                    "--mount",
                    "--uts",
                    "--ipc",
                    "--net",
                    "--pid",
                    &pyexec_output_str,
                    "-m",
                    "pip",
                    "show",
                    "torch",
                ])
                .output();

            let pip_output = match nsenter_pip_output {
                Ok(out) if out.status.success() => Ok(out),
                _ => Command::new(&pyexec_output_str)
                    .args(&["-m", "pip", "show", "torch"])
                    .output(),
            };

            if let Ok(output) = pip_output {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        if line.starts_with("Version:") {
                            let version = line.split(":").nth(1).unwrap_or("").trim();
                            if !version.is_empty() {
                                if check_version_in_range(version, config).is_ok() {
                                    valid_pids.push(*pid);
                                    success = true;
                                    break;
                                } else {
                                    error_message = format!(
                                        "PyTorch version {} not in required range",
                                        version
                                    );
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if !success && error_message.is_empty() {
                let nsenter_torch_output = Command::new("nsenter")
                    .args(&[
                        "--target",
                        &pid.to_string(),
                        "--mount",
                        "--uts",
                        "--ipc",
                        "--net",
                        "--pid",
                        &pyexec_output_str,
                        "-c",
                        "import torch; print(torch.__version__)",
                    ])
                    .output();

                let torch_output = match nsenter_torch_output {
                    Ok(out) if out.status.success() => Ok(out),
                    _ => Command::new(&pyexec_output_str)
                        .args(&["-c", "import torch; print(torch.__version__)"])
                        .output(),
                };

                if let Ok(output) = torch_output {
                    if output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let version_str = stdout.trim();
                        if !version_str.is_empty() {
                            let clean_version = version_str
                                .split('+')
                                .next()
                                .unwrap_or(&version_str)
                                .to_string();
                            if check_version_in_range(&clean_version, config).is_ok() {
                                valid_pids.push(*pid);
                                success = true;
                            } else {
                                error_message = format!(
                                    "PyTorch version {} not in required range",
                                    clean_version
                                );
                            }
                        } else {
                            error_message =
                                "Failed to get PyTorch version from import".to_string();
                        }
                    } else {
                        error_message = format!(
                            "python -c import torch failed: {}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                }
            }
        } else {
            error_message = "Failed to get Python executable path".to_string();
        }

        if !success && !error_message.is_empty() {
            let failed_indicators = vec![
                (
                    IndicatorName::GPU,
                    CollectorName::Pyki,
                    error_message.clone(),
                ),
                (IndicatorName::Torch, CollectorName::Pyki, error_message),
            ];

            for (indicator, collector, reason) in failed_indicators {
                meta_entry.registor_indicator(&indicator, &collector, "Failed", &reason);
            }
        }
    }

    if args.pids.is_empty() && valid_pids.is_empty() {
        let device_process = list_gpu_processes().unwrap_or_else(|e| {
            debug!(
                "Fallback list_gpu_processes() in detect_torch_support failed: {}",
                e
            );
            Vec::new()
        });
        let set1: HashSet<i32> = device_process.into_iter().collect();
        let set2: HashSet<i32> = args.pids.clone().into_iter().collect();
        let intersection: Vec<i32> = set1.intersection(&set2).copied().collect();
        if !intersection.is_empty() {
            log::warn!(
                "Some processes are running on GPU but not detected by Torch: {:?}",
                intersection
            );
            return Ok(intersection);
        }
    }

    Ok(valid_pids)
}

/// Check whether the CUDA version is within the required range.
// Returns the list of PIDs that satisfy the condition.
pub fn detect_cuda_support(
    config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    let mut all_failed = true;
    let mut error_message = String::new();

    let output = Command::new("nsenter")
        .args(&[
            "--target",
            "1",
            "--mount",
            "--uts",
            "--ipc",
            "--net",
            "--pid",
            "nvcc",
            "--version",
        ])
        .output()
        .ok();

    if let Some(output) = output {
        if output.status.success() {
            let version_str = String::from_utf8_lossy(&output.stdout);
            for line in version_str.lines() {
                if line.contains("release") {
                    if let Some(start) = line.find("release ") {
                        let version_part = &line[start + 8..];
                        if let Some(end) = version_part.find(',') {
                            let version = &version_part[..end];
                            if check_version_in_range(version, config).is_ok() {
                                all_failed = false;
                                for pid in &args.pids {
                                    let _ = meta.entry(*pid).or_insert_with(Meta::new);
                                }
                                return Ok(args.pids.clone());
                            } else {
                                error_message =
                                    format!("CUDA version {} not in required range", version);
                            }
                        }
                    }
                }
            }
        } else {
            debug!("nvcc not found");
        }
    }

    let output = Command::new("nsenter")
        .args(&[
            "--target",
            "1",
            "--mount",
            "--uts",
            "--ipc",
            "--net",
            "--pid",
            "nvidia-smi",
            "-q",
        ])
        .output()
        .ok();

    if let Some(output) = output {
        if output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            for line in output_str.lines() {
                if line.trim().starts_with("CUDA Version") {
                    if let Some(version) = line.split(':').nth(1) {
                        let version = version.trim();
                        if check_version_in_range(version, config).is_ok() {
                            all_failed = false;
                            for pid in &args.pids {
                                let _ = meta.entry(*pid).or_insert_with(Meta::new);
                            }
                            return Ok(args.pids.clone());
                        } else {
                            error_message =
                                format!("CUDA version {} not in required range", version);
                        }
                    }
                }
            }
        }
    }

    for pid in &args.pids {
        let maps_path = format!("/proc/{}/maps", pid);
        if let Ok(maps_content) = std::fs::read_to_string(&maps_path) {
            if maps_content.contains("cuda") {
                all_failed = false;
                let _ = meta.entry(*pid).or_insert_with(Meta::new);
                return Ok(args.pids.clone());
            } else {
                error_message = "The process does not reference cuda".to_string();
            }
        }
    }

    if all_failed {
        for pid in &args.pids {
            let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
            meta_entry.registor_indicator(
                &IndicatorName::GPU,
                &CollectorName::Pyki,
                "Failed",
                if error_message.is_empty() {
                    "CUDA not found or version not in required range"
                } else {
                    &error_message
                },
            );
        }
    }

    Ok(vec![])
}

pub fn detect_nvidia_driver(
    config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    let output = Command::new("nvidia-smi")
        .arg("--query-gpu=driver_version")
        .arg("--format=csv,noheader,nounits")
        .output()
        .map_err(|e| {
            ErrorCode::DetectEnvError(None)
                .with_details(format!("Failed to execute nvidia-smi command: {}", e))
                .into_error()
        })?;

    if !output.status.success() {
        fill_meta_for_nvidia_driver_failure(meta, args, "Failed to execute nvidia-smi command");

        return Err(
            ErrorCode::DetectEnvError(Some("nvidia-smi command failed".to_string())).into_error(),
        );
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    let mut success = false;
    let mut error_message = String::new();

    if let Some(version_line) = output_str.lines().next() {
        let version = version_line.trim();
        if check_version_in_range(version, config).is_ok() {
            success = true;
        } else {
            error_message = format!("NVIDIA driver version {} not in required range", version);
        }
    } else {
        error_message = "Failed to parse NVIDIA driver version from nvidia-smi output".to_string();
    }

    if success {
        Ok(args.pids.clone())
    } else {
        fill_meta_for_nvidia_driver_failure(meta, args, &error_message);

        Err(ErrorCode::DetectEnvError(Some(error_message.to_string())).into_error())
    }
}

fn fill_meta_for_nvidia_driver_failure(
    meta: &mut HashMap<i32, Meta>,
    args: &ProfileArgs,
    _error_message: &str,
) {
    for pid in &args.pids {
        let _ = meta.entry(*pid).or_insert_with(Meta::new);
    }
}

pub fn detect_os_support(
    config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    debug!("detect_os_support config is {:?}", config);

    let uname_output = Command::new("uname").arg("-rs").output().ok();

    let mut success = false;
    let mut error_message = String::new();

    if let Some(output) = uname_output {
        if output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let parts: Vec<&str> = output_str.split_whitespace().collect();

            if parts.len() >= 2 {
                let os_name = parts[0].to_string();
                let os_version = parts[1].to_string();

                if let Some(required_os_name) = config.params.get("os_name") {
                    if let Some(required_name) = required_os_name.as_str() {
                        if !os_name.contains(required_name) {
                            error_message = format!(
                                "OS name {} does not match required {}",
                                os_name, required_name
                            );
                        }
                    }
                }

                if error_message.is_empty() {
                    if check_version_in_range(&os_version, config).is_ok() {
                        success = true;
                    } else {
                        error_message =
                            format!("OS version {} not in required range", os_version);
                    }
                }
            }
        }
    }

    if success {
        for pid in &args.pids {
            let _ = meta.entry(*pid).or_insert_with(Meta::new);
        }
        Ok(args.pids.clone())
    } else {
        let _ = error_message;
        for pid in &args.pids {
            let _ = meta.entry(*pid).or_insert_with(Meta::new);
        }
        Ok(vec![])
    }
}

/// Check whether a version string is within the required range.
fn check_version_in_range(version: &str, config: &DependencyConfig) -> Result<()> {
    if let Some(min_version) = config.params.get("min_version") {
        if let Some(min_ver) = min_version.as_str() {
            if compare_versions(version, min_ver) < 0 {
                return Err(ErrorCode::DetectEnvError(None)
                    .with_details(format!(
                        "Version {} is lower than minimum required version {}",
                        version, min_ver
                    ))
                    .into_error());
            }
        }
    }

    if let Some(max_version) = config.params.get("max_version") {
        if let Some(max_ver) = max_version.as_str() {
            if compare_versions(version, max_ver) > 0 {
                return Err(ErrorCode::DetectEnvError(None)
                    .with_details(format!(
                        "Version {} is higher than maximum allowed version {}",
                        version, max_ver
                    ))
                    .into_error());
            }
        }
    }

    Ok(())
}

/// Check whether the process links libpthread or a dynamic libc >= 2.34.
pub fn detect_inject(
    _config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    let mut valid_pids = Vec::new();

    for pid in &args.pids {
        let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);

        let ldd_output = Command::new("nsenter")
            .args(&[
                "--target",
                "1",
                "--mount",
                "--uts",
                "--ipc",
                "--net",
                "--pid",
                "ldd",
                &format!("/proc/{}/exe", pid),
            ])
            .output()
            .map_err(|e| {
                ErrorCode::DetectEnvError(None)
                    .with_details(format!(
                        "Failed to execute ldd command for PID {}: {}",
                        pid, e
                    ))
                    .into_error()
            })?;

        let mut success = false;
        let mut error_message = String::new();

        if ldd_output.status.success() {
            let output_str = String::from_utf8_lossy(&ldd_output.stdout);

            let has_libpthread = output_str.contains("libpthread");

            let mut has_new_libc = false;
            for line in output_str.lines() {
                if line.contains("libc.so") {
                    if let Some(start) = line.find("libc.so.6 =>") {
                        let version_part = &line[start + 12..];
                        if let Some(version_start) = version_part.find('(') {
                            let path_part = &version_part[..version_start].trim();
                            if let Some(version) = extract_libc_version(path_part) {
                                if compare_versions(&version, "2.34") >= 0 {
                                    has_new_libc = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if has_libpthread || has_new_libc {
                valid_pids.push(*pid);
                success = true;
            } else {
                error_message =
                    "Process not linked with libpthread or libc version < 2.34".to_string();
            }
        } else {
            error_message = format!("Failed to execute ldd command for PID {}", pid);
        }

        if !success {
            meta_entry.registor_indicator(
                &IndicatorName::PyStack,
                &CollectorName::Pyki,
                "Failed",
                &error_message,
            );
        }
    }

    Ok(valid_pids)
}

/// Extract the version string from a libc path.
fn extract_libc_version(path: &str) -> Option<String> {
    use std::path::Path;

    let path_obj = Path::new(path);
    if let Some(file_name) = path_obj.file_name() {
        let file_name_str = file_name.to_string_lossy();
        if file_name_str.starts_with("libc-") && file_name_str.ends_with(".so") {
            let version_part = &file_name_str[5..file_name_str.len() - 3];
            return Some(version_part.to_string());
        } else if file_name_str.starts_with("libc.so.") {
            if let Ok(real_path) = std::fs::read_link(path) {
                return extract_libc_version(&real_path.to_string_lossy());
            }
        }
    }
    None
}

/// Compare two version strings.
fn compare_versions(version1: &str, version2: &str) -> i32 {
    let v1_parts: Vec<&str> = version1
        .split(|c| c == '.' || c == '+' || c == '-')
        .collect();
    let v2_parts: Vec<&str> = version2
        .split(|c| c == '.' || c == '+' || c == '-')
        .collect();

    let min_len = v1_parts.len().min(v2_parts.len());

    for i in 0..min_len {
        let v1_num = v1_parts.get(i).unwrap_or(&"0").parse::<i32>().unwrap_or(0);
        let v2_num = v2_parts.get(i).unwrap_or(&"0").parse::<i32>().unwrap_or(0);

        match v1_num.cmp(&v2_num) {
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Equal => continue,
        }
    }

    0
}

/// Check whether pip is available inside the process.
// Returns the list of PIDs that satisfy the condition.
pub fn detect_pip_support(
    _config: &DependencyConfig,
    args: &ProfileArgs,
    meta: &mut HashMap<i32, Meta>,
) -> Result<Vec<i32>> {
    let mut valid_pids = Vec::new();

    for pid in &args.pids {
        let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);

        let pyexec_output_str = get_python_executable(*pid).unwrap_or_default();

        let mut success = false;
        let mut error_message = String::new();

        if !pyexec_output_str.is_empty() {
            let nsenter_output = Command::new("nsenter")
                .args(&[
                    "--target",
                    &pid.to_string(),
                    "--mount",
                    "--uts",
                    "--ipc",
                    "--net",
                    "--pid",
                    &pyexec_output_str,
                    "-m",
                    "pip",
                    "--version",
                ])
                .output();

            let pip_check_output = match nsenter_output {
                Ok(out) if out.status.success() => Ok(out),
                _ => Command::new(&pyexec_output_str)
                    .args(&["-m", "pip", "--version"])
                    .output(),
            };

            match pip_check_output {
                Ok(output) if output.status.success() => {
                    valid_pids.push(*pid);
                    success = true;
                }
                _ => {
                    error_message = "pip not found in the process environment".to_string();
                }
            }
        } else {
            error_message = "Failed to get Python executable path".to_string();
        }

        if !success {
            meta_entry.registor_indicator(
                &IndicatorName::Torch,
                &CollectorName::Pyki,
                "Failed",
                &error_message,
            );
        }
    }

    Ok(valid_pids)
}
