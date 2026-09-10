use crate::error::ErrorCode;
use crate::r#const;
use anyhow::Result;
use std::ffi::{CStr, CString};
use std::fs;
use std::io;
use std::os::raw::c_char;
use std::path::Path;
use std::process::Command;
use std::str::Utf8Error;
use tracing as log;
use glob::glob;

pub trait ToCStr {
    fn to_cstring(&self) -> CString;
    fn as_c_char_ptr_raw(&mut self) -> *mut c_char;
}

#[allow(dead_code)]
pub trait FromCStr {
    fn from_c_str<'a>(ptr: *const c_char) -> Result<&'a str, Utf8Error>;
}

impl ToCStr for String {
    fn to_cstring(&self) -> CString {
        CString::new(self.as_str()).expect("CString::new failed")
    }

    fn as_c_char_ptr_raw(&mut self) -> *mut c_char {
        self.to_cstring().into_raw()
    }
}

impl FromCStr for *const c_char {
    fn from_c_str<'a>(ptr: *const c_char) -> Result<&'a str, Utf8Error> {
        unsafe {
            let c_str = CStr::from_ptr(ptr);
            c_str.to_str()
        }
    }
}

#[macro_export]
macro_rules! include_header {
    ($package: tt, $alias: ident) => {
        #[allow(
            non_camel_case_types,
            non_snake_case,
            non_upper_case_globals,
            dead_code
        )]
        pub(crate) mod $alias {
            include!(concat!(env!("OUT_DIR"), concat!("/", $package, ".rs")));
        }
    };
}

/// Determine whether a process is running on the host or inside a container.
/// Compare the target process's PID namespace against the host init (PID 1) PID
/// namespace; if they differ, the process is in a container.
/// `true` - process runs in a container; `false` - process runs on the host
pub fn is_process_in_container(pid: i32) -> bool {
    // Build PID namespace paths for the target and for host PID 1
    let target_pid_ns_path = format!("/proc/{}/ns/pid", pid);
    let host_pid_ns_path = "/proc/1/ns/pid";
    // Try to read the PID namespace symlinks of both processes
    let ns_check = match (
        std::fs::read_link(&target_pid_ns_path),
        std::fs::read_link(host_pid_ns_path),
    ) {
        (Ok(target_ns), Ok(host_ns)) => {
            // Different PID namespaces indicate the process is in a container
            target_ns != host_ns
        }
        _ => {
            // If either namespace cannot be read, assume the process runs on the host
            false
        }
    };

    // If the PID namespace check already confirmed a container, return true
    if ns_check {
        return true;
    }

    // If PID namespaces match, use extra checks to determine container residency
    // Inspect the cgroup information
    let cgroup_path = format!("/proc/{}/cgroup", pid);
    if let Ok(cgroup_content) = std::fs::read_to_string(&cgroup_path) {
        // Look for container-related cgroup keywords
        for line in cgroup_content.lines() {
            if line.contains("/docker/")
                || line.contains("/containerd/")
                || line.contains("/lxc/")
                || line.contains("/crio/")
                || line.contains("/kubepods/")
            {
                // Fine-grained match for the k8s format
                return true;
            }
        }
    }

    // Inspect the mounts information
    let mounts_path = format!("/proc/{}/mounts", pid);
    if let Ok(mounts_content) = std::fs::read_to_string(&mounts_path) {
        // Look for container-specific mount points
        for line in mounts_content.lines() {
            if line.contains("/docker/containers/")
                || line.contains("/var/lib/docker/")
                || line.contains("/var/lib/containerd/")
                || line.contains("/var/lib/lxc/")
            {
                return true;
            }
        }
    }

    // No container signals detected; treat as host process
    false
}

pub fn is_root() -> bool {
    // On Unix/Linux, the root user's UID is 0.
    // Check the current process's effective UID.
    unsafe { libc::geteuid() == 0 }
}


fn extract_virtual_env(pid: i32) -> Option<String> {
    let environ_path = format!("/proc/{}/environ", pid);
    let bytes = fs::read(&environ_path).ok()?;
    let entries = bytes.split(|&b| b == 0);
    for entry in entries {
        if entry.is_empty() {
            continue;
        }
        let entry_str = String::from_utf8_lossy(entry);
        if let Some(path) = entry_str.strip_prefix("VIRTUAL_ENV=") {
            let venv_path = path.to_string();
            if !venv_path.is_empty() {
                log::debug!(
                    "Found VIRTUAL_ENV='{}' from /proc/{}/environ",
                    venv_path, pid
                );
                return Some(venv_path);
            }
        }
    }
    log::debug!("VIRTUAL_ENV not found in /proc/{}/environ", pid);
    None
}

pub fn get_python_executable(pid: i32) -> Result<String> {
    // 1. Prefer resolving the python path via the VIRTUAL_ENV environment variable
    if let Some(venv_path) = extract_virtual_env(pid) {
        let venv_python = format!("{}/bin/python", venv_path);
        let direct_exists = Path::new(&venv_python).exists();
        let ns_path = format!("/proc/{}/root{}", pid, &venv_python);
        let ns_exists = Path::new(&ns_path).exists();

        if direct_exists || ns_exists {
            log::debug!(
                "Using VIRTUAL_ENV python path: '{}' (direct={}, ns={})",
                venv_python, direct_exists, ns_exists
            );
            return Ok(venv_python);
        } else {
            log::debug!(
                "VIRTUAL_ENV python path '{}' does not exist (checked direct and /proc/{}/root), falling back to readlink",
                venv_python, pid
            );
        }
    }

    // 2. Fallback: try `readlink -f` to get the absolute path
    let mut pyexec = Command::new("readlink")
        .args(&["-f", &format!("/proc/{}/exe", pid)])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // 3. If `readlink -f` failed, try plain `readlink`
    if pyexec.is_empty() {
        pyexec = Command::new("readlink")
            .arg(format!("/proc/{}/exe", pid))
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();
    }

    if pyexec.is_empty() {
        return Err(ErrorCode::CommandFailed(Some(format!(
            "Failed to get python executable for PID {}",
            pid
        )))
        .into_error());
    }

    Ok(pyexec)
}

/// Copy a file from `src` to `dst`
pub fn copy_file(src: &str, dst: &str, delete: bool) -> Result<()> {
    // Check whether the source path contains glob wildcards
    if src.contains('*') || src.contains('?') || src.contains('[') {
        // Handle wildcards via glob
        let mut copied_files = 0;
        for entry in glob(src)? {
            match entry {
                Ok(path) => {
                    // Build the destination path
                    let file_name = path
                        .file_name()
                        .unwrap_or_else(|| std::ffi::OsStr::new("unknown"));
                    let dst_path = Path::new(dst).join(file_name);

                    copy_recursive(&path, &dst_path, delete)?;
                    copied_files += 1;
                }
                Err(e) => {
                    log::error!("Wildcard paths cannot be processed {}: {}", src, e);
                    return Err(ErrorCode::CopyFilesError(Some(format!(
                        "Wildcard paths cannot be processed {}: {}",
                        src, e
                    )))
                    .into_error());
                }
            }
        }

        if copied_files == 0 {
            return Err(ErrorCode::CopyFilesError(Some(format!(
                "No file matching the wildcard path was found: {}",
                src
            )))
            .into_error());
        }
    } else {
        // Original path (no wildcards)
        copy_recursive(src, dst, delete)?;
        log::debug!("Copy from {} to {}", src, dst);
    }
    Ok(())
}

/// Build the glob for one target's own artifacts inside its `/tmp`.
///
/// The delimiter is not cosmetic. `AIProf_<pid>*` also matches a longer pid
/// whose decimal form merely starts with the same digits - for pid 123 it
/// matches `AIProf_1234_cupti.json` - and these globs drive a *destructive*
/// move, so an over-match steals and unlinks another target's trace. Both
/// producers put a separator immediately after the pid: cuprof writes
/// `AIProf_<pid>_cupti.json` (see cupti_plugin_wrapper::custom_trigger) and
/// pyki writes `AIProf_<pid>-<ns-pid>-<id>-<suffix>` (its prefix is passed as
/// `AIProf_<pid>`, see pyki/profiling/torch_profile.py::gen_result_path). So
/// requiring the separator removes the ambiguity with no under-match risk.
pub fn pid_artifact_pattern(pid: i32, delimiter: char, suffix: &str) -> String {
    // DEFAULT_PATH is "/tmp" with no trailing slash, so the separator before
    // DEFAULT_PREFIX has to be written out explicitly - omitting it yields
    // "/proc/<pid>/root/tmpAIProf_<pid>...", which silently matches nothing.
    format!(
        "/proc/{}/root{}/{}{}{}{}",
        pid,
        r#const::DEFAULT_PATH,
        r#const::DEFAULT_PREFIX,
        pid,
        delimiter,
        suffix
    )
}

/// Move every file matching `pattern` into directory `dst`, returning how many
/// were moved.
///
/// Same semantics as `copy_file(.., delete=true)` for the wildcard case, except
/// that an empty match set is `Ok(0)` rather than an error. Callers must be able
/// to tell "nothing to collect yet" apart from a genuine I/O failure in order to
/// log each at the right level; `copy_file` collapses both into `Err`, which is
/// why these call sites used to swallow everything with `let _ =`.
pub fn move_matching(pattern: &str, dst: &str) -> Result<usize> {
    let mut moved = 0;
    for entry in glob(pattern)? {
        let path = match entry {
            Ok(path) => path,
            Err(e) => {
                log::error!("Wildcard paths cannot be processed {}: {}", pattern, e);
                return Err(ErrorCode::CopyFilesError(Some(format!(
                    "Wildcard paths cannot be processed {}: {}",
                    pattern, e
                )))
                .into_error());
            }
        };
        let file_name = path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("unknown"));
        let dst_path = Path::new(dst).join(file_name);
        copy_recursive(&path, &dst_path, true)?;
        moved += 1;
    }
    Ok(moved)
}

pub fn get_pip_version(pyexec: &str, pid: &str) -> Result<String> {
    // Prefer nsenter
    let nsenter_output = Command::new("nsenter")
        .args([
            "--target",
            pid,
            "--mount",
            "--uts",
            "--ipc",
            "--net",
            "--pid",
            pyexec,
            "-m",
            "pip",
            "--version",
        ])
        .output();

    // If nsenter fails, fall back to direct execution
    let output = match nsenter_output {
        Ok(out) if out.status.success() => Ok(out),
        _ => {
            Command::new(pyexec)
                .args(["-m", "pip", "--version"])
                .output()
        }
    }?;

    if !output.status.success() {
        return Err(
            ErrorCode::CommandFailed(Some("Failed to get pip version".to_string())).into_error(),
        );
    }

    let version_output = String::from_utf8(output.stdout)?;
    let parts: Vec<&str> = version_output.split_whitespace().collect();
    if parts.len() >= 2 {
        Ok(parts[1].to_string())
    } else {
        return Err(
            ErrorCode::CommandFailed(Some("Failed to get pip version".to_string())).into_error(),
        );
    }
}

pub fn is_pip_version_supported_for_break_system_packages(version: &str) -> Result<bool> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 {
        return Ok(false);
    }

    let major: i32 = parts[0].parse()?;
    let minor: i32 = parts[1].parse()?;

    Ok(major > 23 || (major == 23 && minor >= 3))
}

pub fn is_pyki_installed(pyexec: &str, pid: &str) -> Result<bool> {
    let nsenter_output = Command::new("nsenter")
        .args([
            "--target", pid, "--mount", "--uts", "--ipc", "--net", "--pid", pyexec, "-m", "pip",
            "show", "pyki",
        ])
        .output();

    let output = match nsenter_output {
        Ok(out) if out.status.success() => {
            out
        },
        Ok(out) => {
            out
        },
        Err(_e) => {
            match Command::new(pyexec)
                .args(["-m", "pip", "show", "pyki"])
                .output() {
                Ok(out) => {
                    out
                },
                Err(e) => {
                    // Direct execution also failed, assume pyki is not installed
                    log::error!("direct execution failed: {:?}, assuming pyki not installed", e);
                    return Ok(false);
                }
            }
        }
    };

    Ok(output.status.success())
}

fn copy_recursive<P: AsRef<Path>, Q: AsRef<Path>>(
    source: P,
    destination: Q,
    delete: bool,
) -> Result<()> {
    let source = source.as_ref();
    let destination = destination.as_ref();

    if !source.exists() {
        return Err(ErrorCode::CopyFilesError(Some(format!(
            "The source path does not exist: {}",
            source.display()
        )))
        .into_error());
    }

    // If the source is a file, copy it directly
    if source.is_file() {
        // Create the parent directory if it does not exist
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        if delete {
            fs::remove_file(source)?;
        }
        return Ok(());
    }

    // If the source is a directory, copy it recursively
    if source.is_dir() {
        // Create the destination directory
        fs::create_dir_all(destination)?;

        // Iterate over all entries in the source directory
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let entry_path = entry.path();
            let relative_path = entry_path
                .strip_prefix(source)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            let dest_path = destination.join(relative_path);

            if entry_path.is_file() {
                fs::copy(&entry_path, &dest_path)?;
                if delete {
                    fs::remove_file(source)?;
                }
            } else if entry_path.is_dir() {
                copy_recursive(&entry_path, &dest_path, delete)?;
            }
        }
        Ok(())
    } else {
        Err(ErrorCode::CopyFilesError(Some(format!(
            "Unsupported file types: {}",
            source.display()
        )))
        .into_error())
    }
}

pub fn get_installed_pyki_version(pyexec: &str, pid: &str) -> Result<String> {
    let nsenter_output = Command::new("nsenter")
        .args([
            "--target", pid, "--mount", "--uts", "--ipc", "--net", "--pid", pyexec, "-m", "pip",
            "show", "pyki",
        ])
        .output();

    let output = match nsenter_output {
        Ok(out) if out.status.success() => Ok(out),
        _ => {
            Command::new(pyexec)
                .args(["-m", "pip", "show", "pyki"])
                .output()
        }
    }?;

    if !output.status.success() {
        return Err(ErrorCode::CommandFailed(None).into_error());
    }

    let output_str = String::from_utf8(output.stdout)?;
    for line in output_str.lines() {
        if line.starts_with("Version:") {
            return Ok(line.split(':').nth(1).unwrap_or("").trim().to_string());
        }
    }

    Err(ErrorCode::CommandFailed(None).into_error())
}

pub fn get_release_pyki_version() -> Result<String> {
    let version = r#const::PYKI_VERSION_IN;
    Ok(version.to_string())
}

pub fn install_pyki(pyexec: &str, break_system_packages: bool, pid: &str) -> Result<()> {
    let mut args = vec![
        "--target",
        pid,
        "--mount",
        "--uts",
        "--ipc",
        "--net",
        "--pid",
        pyexec,
        "-m",
        "pip",
        "install",
        "pyki",
        "--find-links=file:///pyki_dir/pyki",
        "--no-index",
    ];

    if break_system_packages {
        args.push("--break-system-packages");
    }

    let nsenter_output = Command::new("nsenter").args(&args).output();

    let output = match nsenter_output {
        Ok(out) if out.status.success() => Ok(out),
        _ => {
            let mut direct_args = vec!["-m", "pip", "install", "pyki", "--find-links=file:///pyki_dir/pyki", "--no-index"];
            if break_system_packages {
                direct_args.push("--break-system-packages");
            }
            Command::new(pyexec).args(&direct_args).output()
        }
    }?;

    if !output.status.success() {
        return Err(ErrorCode::PykiInstallFailed(Some(format!(
            "{:#?}",
            String::from_utf8(output.stderr)
        )))
        .into_error());
    }

    Ok(())
}

pub fn uninstall_pyki(pyexec: &str, break_system_packages: bool, pid: &str) -> Result<()> {
    let mut args = vec![
        "--target",
        pid,
        "--mount",
        "--uts",
        "--ipc",
        "--net",
        "--pid",
        pyexec,
        "-m",
        "pip",
        "uninstall",
        "pyki",
        "-y",
    ];

    if break_system_packages {
        args.push("--break-system-packages");
    }

    let nsenter_output = Command::new("nsenter").args(&args).output();

    let output = match nsenter_output {
        Ok(out) if out.status.success() => Ok(out),
        _ => {
            let mut direct_args = vec!["-m", "pip", "uninstall", "pyki", "-y"];
            if break_system_packages {
                direct_args.push("--break-system-packages");
            }
            Command::new(pyexec).args(&direct_args).output()
        }
    }?;

    if !output.status.success() {
        return Err(ErrorCode::PykiUninstallFailed(Some(format!(
            "{:#?}",
            String::from_utf8(output.stderr)
        )))
        .into_error());
    }

    Ok(())
}

/// Detect the Linux distribution family
pub fn detect_distro_type() -> Result<&'static str> {
    // Check for /etc/redhat-release (CentOS, RHEL, Fedora)
    if std::path::Path::new("/etc/redhat-release").exists() {
        return Ok("rpm");
    }

    // Check for /etc/debian_version (Debian, Ubuntu)
    if std::path::Path::new("/etc/debian_version").exists() {
        return Ok("deb");
    }

    // Inspect /etc/os-release
    if let Ok(os_release) = std::fs::read_to_string("/etc/os-release") {
        if os_release.contains("ID_LIKE=debian")
            || os_release.contains("ID=debian")
            || os_release.contains("ID=ubuntu")
        {
            return Ok("deb");
        }
        if os_release.contains("ID_LIKE=redhat")
            || os_release.contains("ID=alinux")
            || os_release.contains("ID=rhel")
            || os_release.contains("ID=fedora")
            || os_release.contains("ID=centos")
        {
            return Ok("rpm");
        }
    }

    // Fall back to the `lsblk` command
    let output = Command::new("lsblk").output();
    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}\n{}", stdout, stderr);

        if combined.contains("debian") || combined.contains("ubuntu") {
            return Ok("deb");
        }
        if combined.contains("redhat") || combined.contains("fedora") || combined.contains("centos")
        {
            return Ok("rpm");
        }
    }

    Err(anyhow::anyhow!("unable to detect linux distribution"))
}

/// Convert a host PID to the container-internal PID
/// Equivalent shell: grep "NSpid" /proc/$p/status | awk '{print $3}' | tr -d '\n'
pub fn convert_host_pid_to_container_pid(host_pid: i32) -> Result<i32, String> {
    let status_path = format!("/proc/{}/status", host_pid);

    // Read the status file
    let content = std::fs::read_to_string(&status_path)
        .map_err(|e| format!("Failed to read {}: {}", status_path, e))?;

    // Find the NSpid line and extract the container-side PID
    for line in content.lines() {
        if line.starts_with("NSpid:") {
            // NSpid line format is like: NSpid:\t25360\t123
            // The last number is the container-internal PID
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                // Try to parse the last token as the container PID
                if let Some(container_pid_str) = parts.last() {
                    if let Ok(container_pid) = container_pid_str.parse::<i32>() {
                        return Ok(container_pid);
                    }
                }
            }
            return Err("Failed to parse container PID from NSpid line".to_string());
        }
    }

    Err("NSpid line not found in status file".to_string())
}

/// Check whether the process exists
pub fn process_exists(pid: i32) -> bool {
    // Test whether /proc/{pid} exists
    let proc_path = format!("/proc/{}", pid);
    std::path::Path::new(&proc_path).exists()
}

/// Check whether a PID is valid: process exists and resources are sufficient
pub fn is_pid_valid(config: &crate::config::Config, pid: i32) -> bool {
    // First verify that the process exists
    if !process_exists(pid) {
        log::error!("Process {} does not exist", pid);
        return false;
    }

    // Then verify that resources are sufficient
    match crate::tools::detect_resource::detect_resource(config, &pid) {
        Ok(true) => {
            log::debug!("Process {} exists and has sufficient resources", pid);
            true
        }
        Ok(false) => {
            log::error!("Process {} does not meet resource requirements", pid);
            false
        }
        Err(e) => {
            log::error!("Resource detection failed for process {}: {}", pid, e);
            false
        }
    }
}

/// Remove every file starting with r#const::DEFAULT_PREFIX in the given directory.
/// # Arguments
/// * `dir_path` - target directory path
///
/// # Returns
/// * `Ok(usize)` - number of files successfully deleted
/// * `Err(anyhow::Error)` - error information on deletion failure
/// ```
pub fn clean_aiprof_files<P: AsRef<Path>>(dir_path: P) -> Result<usize> {
    let dir_path = dir_path.as_ref();

    // Ensure the directory exists
    if !dir_path.exists() {
        return Err(ErrorCode::CopyFilesError(Some(format!(
            "Directory does not exist: {}",
            dir_path.display()
        )))
        .into_error());
    }

    // Ensure the path is a directory
    if !dir_path.is_dir() {
        return Err(ErrorCode::CopyFilesError(Some(format!(
            "Path is not a directory: {}",
            dir_path.display()
        )))
        .into_error());
    }

    let mut deleted_count = 0;

    // Iterate over every entry in the directory
    for entry in fs::read_dir(dir_path)? {
        let entry = entry?;
        let path = entry.path();

        // Extract the file name
        if let Some(file_name) = path.file_name() {
            if let Some(file_name_str) = file_name.to_str() {
                // Check whether the file name starts with AIProf_
                if file_name_str.starts_with(r#const::DEFAULT_PREFIX) {
                    if path.is_file() {
                        match fs::remove_file(&path) {
                            Ok(_) => {
                                log::debug!("Deleted file: {}", path.display());
                                deleted_count += 1;
                            }
                            Err(e) => {
                                log::error!("Failed to delete file {}: {}", path.display(), e);
                                return Err(ErrorCode::CopyFilesError(Some(format!(
                                    "Failed to delete file {}: {}",
                                    path.display(),
                                    e
                                )))
                                .into_error());
                            }
                        }
                    } else {
                        log::debug!("Skipping non-file entry: {}", path.display());
                    }
                }
            }
        }
    }

    log::debug!(
        "Cleaned {} {} files from {}",
        deleted_count,
        r#const::DEFAULT_PREFIX,
        dir_path.display()
    );
    Ok(deleted_count)
}

// ---- minimal ELF64 dynamic-section reader ------------------------------
// Only the DT_SONAME / DT_NEEDED tags are needed to match the vendored libcupti,
// so it is not worth pulling in an ELF crate; a narrow hand-rolled little-endian
// ELF64 parser suffices. The runtime image is not guaranteed to ship
// readelf/objdump, so shelling out is not an option either.

const DT_NEEDED: u64 = 1;
const DT_SONAME: u64 = 14;
const DT_STRTAB: u64 = 5;
const DT_STRSZ: u64 = 10;
const DT_NULL: u64 = 0;

fn read_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn read_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}
fn read_u64(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8)
        .map(|s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

// Map a virtual address back to a file offset using the PT_LOAD program headers.
fn vaddr_to_off(buf: &[u8], phoff: u64, phentsize: u16, phnum: u16, vaddr: u64) -> Option<u64> {
    for i in 0..phnum as u64 {
        let base = (phoff + i * phentsize as u64) as usize;
        let p_type = read_u32(buf, base)?;
        if p_type != 1 {
            // PT_LOAD
            continue;
        }
        let p_offset = read_u64(buf, base + 8)?;
        let p_vaddr = read_u64(buf, base + 16)?;
        let p_filesz = read_u64(buf, base + 32)?;
        if vaddr >= p_vaddr && vaddr < p_vaddr + p_filesz {
            return Some(p_offset + (vaddr - p_vaddr));
        }
    }
    None
}

// Parse the dynamic section and return (soname, needed_list).
fn parse_dynamic(path: &str) -> Result<(Option<String>, Vec<String>)> {
    let buf = fs::read(path)?;
    if buf.len() < 64 || &buf[0..4] != b"\x7fELF" || buf[4] != 2 {
        return Err(ErrorCode::CuptiPluginError(Some(format!(
            "{} is not a 64-bit ELF",
            path
        )))
        .into_error());
    }
    let phoff = read_u64(&buf, 32).ok_or_else(|| bad_elf(path))?;
    let phentsize = read_u16(&buf, 54).ok_or_else(|| bad_elf(path))?;
    let phnum = read_u16(&buf, 56).ok_or_else(|| bad_elf(path))?;

    // Find PT_DYNAMIC (p_type == 2)
    let mut dyn_off = None;
    let mut dyn_size = 0u64;
    for i in 0..phnum as u64 {
        let base = (phoff + i * phentsize as u64) as usize;
        if read_u32(&buf, base) == Some(2) {
            dyn_off = read_u64(&buf, base + 8);
            dyn_size = read_u64(&buf, base + 32).unwrap_or(0);
            break;
        }
    }
    let dyn_off = dyn_off.ok_or_else(|| bad_elf(path))? as usize;

    // First pass: find strtab's virtual address plus size and collect every tag/val pair.
    let mut entries: Vec<(u64, u64)> = Vec::new();
    let mut strtab_vaddr = None;
    let mut strsz = 0u64;
    let entry_count = (dyn_size / 16) as usize;
    for i in 0..entry_count {
        let base = dyn_off + i * 16;
        let tag = read_u64(&buf, base).ok_or_else(|| bad_elf(path))?;
        let val = read_u64(&buf, base + 8).ok_or_else(|| bad_elf(path))?;
        if tag == DT_NULL {
            break;
        }
        if tag == DT_STRTAB {
            strtab_vaddr = Some(val);
        } else if tag == DT_STRSZ {
            strsz = val;
        }
        entries.push((tag, val));
    }
    let strtab_vaddr = strtab_vaddr.ok_or_else(|| bad_elf(path))?;
    let strtab_off = vaddr_to_off(&buf, phoff, phentsize, phnum, strtab_vaddr)
        .ok_or_else(|| bad_elf(path))? as usize;
    let strtab_end = (strtab_off + strsz as usize).min(buf.len());
    let strtab = &buf[strtab_off..strtab_end];

    let read_str = |idx: u64| -> Option<String> {
        let start = idx as usize;
        if start >= strtab.len() {
            return None;
        }
        let end = strtab[start..]
            .iter()
            .position(|&c| c == 0)
            .map(|p| start + p)
            .unwrap_or(strtab.len());
        std::str::from_utf8(&strtab[start..end])
            .ok()
            .map(|s| s.to_string())
    };

    let mut soname = None;
    let mut needed = Vec::new();
    for (tag, val) in entries {
        if tag == DT_SONAME {
            soname = read_str(val);
        } else if tag == DT_NEEDED {
            if let Some(s) = read_str(val) {
                needed.push(s);
            }
        }
    }
    Ok((soname, needed))
}

fn bad_elf(path: &str) -> anyhow::Error {
    ErrorCode::CuptiPluginError(Some(format!("failed to parse ELF dynamic section: {}", path)))
        .into_error()
}

/// Read the DT_SONAME of a shared library (e.g. "libcupti.so.12").
pub fn read_soname(path: &str) -> Result<String> {
    let (soname, _) = parse_dynamic(path)?;
    soname.ok_or_else(|| {
        ErrorCode::CuptiPluginError(Some(format!("{} has no DT_SONAME", path))).into_error()
    })
}

/// Find the first DT_NEEDED entry in `path` whose name starts with `prefix`
/// (e.g. prefix="libcupti.so." returns "libcupti.so.12").
pub fn read_needed_soname(path: &str, prefix: &str) -> Result<String> {
    let (_, needed) = parse_dynamic(path)?;
    needed
        .into_iter()
        .find(|n| n.starts_with(prefix))
        .ok_or_else(|| {
            ErrorCode::CuptiPluginError(Some(format!(
                "{} has no NEEDED entry starting with '{}'",
                path, prefix
            )))
            .into_error()
        })
}

#[cfg(test)]
mod artifact_glob_tests {
    use super::*;
    use glob::glob;

    const NAMES: &[&str] = &[
        "AIProf_123_cupti.json",
        "AIProf_1234_cupti.json",
        "AIProf_9999_cupti.json",
    ];

    fn touch(dir: &Path, names: &[&str]) {
        for n in names {
            std::fs::write(dir.join(n), b"{}").unwrap();
        }
    }

    #[test]
    fn pattern_is_a_well_formed_path_under_the_targets_tmp() {
        // Guards the separator between DEFAULT_PATH ("/tmp", no trailing slash)
        // and DEFAULT_PREFIX: without it the glob is "/root/tmpAIProf_<pid>..."
        // and matches nothing, which would silently empty every pyki report.
        let p = pid_artifact_pattern(1234, '_', "*.json");
        assert!(p.starts_with("/proc/1234/root/tmp/"), "got {}", p);
        assert!(p.contains("/tmp/AIProf_"), "got {}", p);
        assert!(!p.contains("tmpAIProf"), "missing separator: {}", p);
    }

    #[test]
    fn pattern_pins_the_delimiter_right_after_the_pid() {
        assert_eq!(
            pid_artifact_pattern(1234, '_', "*.json"),
            "/proc/1234/root/tmp/AIProf_1234_*.json"
        );
        assert_eq!(
            pid_artifact_pattern(1234, '-', "*cuda-memory.pickle"),
            "/proc/1234/root/tmp/AIProf_1234-*cuda-memory.pickle"
        );
    }

    // Regression for the over-match both inject paths used to have: the sweep is
    // a move, so pid 123 collecting `AIProf_123*` stole and unlinked pid 1234's
    // trace, and pid 1234's own guarded copy then found nothing.
    #[test]
    fn the_unanchored_pattern_demonstrably_over_matches() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), NAMES);
        let loose = format!(
            "{}/{}{}*.json",
            dir.path().display(),
            r#const::DEFAULT_PREFIX,
            123
        );
        let hits: Vec<_> = glob(&loose).unwrap().flatten().collect();
        assert_eq!(
            hits.len(),
            2,
            "documents why the delimiter is required: AIProf_123* also matches pid 1234"
        );
    }

    #[test]
    fn the_anchored_pattern_matches_only_that_pid() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), NAMES);
        let out = tempfile::tempdir().unwrap();
        let anchored = format!(
            "{}/{}{}_*.json",
            dir.path().display(),
            r#const::DEFAULT_PREFIX,
            123
        );

        let moved = move_matching(&anchored, out.path().to_str().unwrap()).unwrap();
        assert_eq!(moved, 1, "exactly one file belongs to pid 123");
        assert!(out.path().join("AIProf_123_cupti.json").exists());
        // move semantics: the source is gone
        assert!(!dir.path().join("AIProf_123_cupti.json").exists());
        // other pids stay put and do not leak into this run's output
        for n in ["AIProf_1234_cupti.json", "AIProf_9999_cupti.json"] {
            assert!(dir.path().join(n).exists(), "{} was stolen", n);
            assert!(!out.path().join(n).exists(), "{} leaked into the output", n);
        }
    }

    #[test]
    fn no_match_is_ok_zero_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let anchored = format!(
            "{}/{}{}_*.json",
            dir.path().display(),
            r#const::DEFAULT_PREFIX,
            4242
        );
        // copy_file reports this case as Err, which is why the call sites could
        // not distinguish it from a real I/O failure.
        assert!(copy_file(&anchored, out.path().to_str().unwrap(), true).is_err());
        assert_eq!(
            move_matching(&anchored, out.path().to_str().unwrap()).unwrap(),
            0
        );
    }
}
