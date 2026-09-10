use crate::injector;
use crate::tools::utils::ToCStr;
use crate::utils;
use crate::{r#const, ErrorCode};
use anyhow::Result;
use lazy_static::lazy_static;
use std::io::{Error, ErrorKind};
use std::sync::Mutex;
use std::{fs, path::Path};
use tracing as log;

lazy_static! {
    static ref INJECTOR_MUTEX: Mutex<()> = Mutex::new(());
}

// Heap-growth / dlopen syscalls on x86_64. If any target thread is inside one
// of these, ptrace-attach lands the target on top of the glibc allocator or
// dynamic-linker locks — exactly the path libprofiler.a's blacklist rejects.
// futex (202) is deliberately excluded: idle workers park there constantly
// and libprofiler only rejects userspace stack frames, not futex parking.
const DANGEROUS_SYSCALLS_X86_64: &[u64] = &[
    9,   // mmap
    10,  // mprotect
    11,  // munmap
    12,  // brk
    56,  // clone
    59,  // execve
];

// Syscalls where the target's main thread has definitely dropped the GIL
// (Python releases the GIL around any blocking syscall via Py_BEGIN_ALLOW_THREADS).
// If the main thread is here, an injected clone thread's PyGILState_Ensure()
// will return promptly instead of hanging on a CUDA-busy interpreter.
const GIL_RELEASING_SYSCALLS_X86_64: &[u64] = &[
    35,  // nanosleep
    230, // clock_nanosleep
    202, // futex (Python's per-interpreter GIL locks park here when waiting)
    7,   // poll
    232, // epoll_wait
    281, // epoll_pwait
    23,  // select
    270, // pselect6
    0,   // read
    1,   // write
    17,  // pread64
    18,  // pwrite64
    247, // waitid
    61,  // wait4
];

// Read /proc/PID/maps and return the address range covered by the dynamic
// linker (ld-linux*.so). glibc's `_dl_load_lock` lives inside this range as
// a static-storage pthread_mutex_t. Returns None if the linker isn't in
// /proc/PID/maps (statically linked target, unreadable procfs, etc.).
fn ld_linux_range(pid: i32) -> Option<(u64, u64)> {
    let content = std::fs::read_to_string(format!("/proc/{}/maps", pid)).ok()?;
    let mut lo = u64::MAX;
    let mut hi = 0u64;
    for line in content.lines() {
        // Match both ld-linux-x86-64.so.2 and ld-2.XX.so naming schemes.
        let is_ld = line.contains("/ld-linux") || line.contains("/ld-2.");
        if !is_ld {
            continue;
        }
        // Line format: "start-end perms offset dev inode  path"
        let range = match line.split_whitespace().next() {
            Some(r) => r,
            None => continue,
        };
        let mut parts = range.split('-');
        let start = parts.next().and_then(|s| u64::from_str_radix(s, 16).ok());
        let end = parts.next().and_then(|s| u64::from_str_radix(s, 16).ok());
        if let (Some(s), Some(e)) = (start, end) {
            lo = lo.min(s);
            hi = hi.max(e);
        }
    }
    if hi == 0 {
        None
    } else {
        Some((lo, hi))
    }
}

// A previous failed injection can leave clone threads permanently parked on
// glibc's `_dl_load_lock` inside `_dl_open`. When that happens, no live
// thread in the target owns the lock — it's simply been abandoned — and
// every subsequent dlopen (including our next libloader.so injection)
// deadlocks against it, so profiler_wait ages out at 60 s and the run
// silently fails. We can't reach libprofiler's internals to fix its detach
// path, but we can detect the condition cheaply: look for two or more
// python threads parked in futex_wait on an address inside the dynamic
// linker's memory range. A single dlopen in progress can produce one such
// thread transiently; two or more concurrent waiters on the same
// linker-internal futex is essentially always the orphan pattern.
//
// Returns Some((futex_addr, count)) when it looks poisoned, None otherwise.
fn detect_orphan_dl_load_lock(pid: i32) -> Option<(u64, usize)> {
    let (ld_lo, ld_hi) = ld_linux_range(pid)?;
    let task_dir = format!("/proc/{}/task", pid);
    let entries = std::fs::read_dir(&task_dir).ok()?;

    use std::collections::HashMap;
    let mut counts: HashMap<u64, usize> = HashMap::new();

    for entry in entries.flatten() {
        let sc = match std::fs::read_to_string(entry.path().join("syscall")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let sc = sc.trim();
        if sc.starts_with("running") {
            continue;
        }
        let mut it = sc.split_whitespace();
        let nr = it.next().and_then(|t| t.parse::<u64>().ok());
        // syscall line: "202 <arg0> <arg1> ..." — for futex, arg0 is the addr.
        if nr != Some(202) {
            continue;
        }
        let addr = match it.next() {
            Some(a) => a.trim_start_matches("0x"),
            None => continue,
        };
        let addr = match u64::from_str_radix(addr, 16) {
            Ok(a) => a,
            Err(_) => continue,
        };
        if addr < ld_lo || addr >= ld_hi {
            continue;
        }
        *counts.entry(addr).or_insert(0) += 1;
    }

    counts
        .into_iter()
        .filter(|(_, n)| *n >= 2)
        .max_by_key(|(_, n)| *n)
}

fn is_dangerous_wchan(w: &str) -> bool {
    // Kernel-space paths where a task is holding a mutex we might block on
    // when we drive dlopen/malloc through the injected thread. Page-fault
    // catches copy-on-write into an anonymous mapping (common right after
    // brk); *map / mprotect catch the mmap syscall being serviced.
    w.contains("brk")
        || w.contains("do_mmap")
        || w.contains("vm_mmap")
        || w.contains("mprotect")
        || w.contains("do_page_fault")
        || w.contains("handle_mm_fault")
}

fn is_dangerous_syscall(line: &str) -> bool {
    // "running" is emitted verbatim by the kernel when the task is in
    // userspace. Cannot pre-detect userspace-only allocator hits; those
    // are handled by libprofiler.a's own retry.
    if line.starts_with("running") {
        return false;
    }
    let nr = line.split_whitespace().next().and_then(|t| t.parse::<u64>().ok());
    match nr {
        Some(n) => DANGEROUS_SYSCALLS_X86_64.contains(&n),
        None => false,
    }
}

// The main Python thread of a target process is task-id == tgid (i.e. /proc/PID
// with the same PID as the process leader). Read its current syscall number and
// return true if that syscall is one where Python has released the GIL —
// meaning an injected PyGILState_Ensure() will succeed quickly instead of
// spinning waiting for a CUDA-busy interpreter to yield.
fn main_thread_released_gil(pid: i32) -> bool {
    let syscall_path = format!("/proc/{}/task/{}/syscall", pid, pid);
    let sc = match std::fs::read_to_string(&syscall_path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let sc = sc.trim();
    if sc.starts_with("running") {
        // Executing pure Python bytecode or a CUDA kernel dispatch — the GIL
        // is held. Not a good moment.
        return false;
    }
    let nr = match sc.split_whitespace().next().and_then(|t| t.parse::<u64>().ok()) {
        Some(n) => n,
        None => return false,
    };
    GIL_RELEASING_SYSCALLS_X86_64.contains(&nr)
}


// Returns Some(reason) if any thread of `pid` is currently in a state that
// makes profiler_attach likely to hit libprofiler.a's blacklist path (which
// on retry can corrupt the target's heap → SIGABRT). Returns None if the
// snapshot looks safe. Best-effort: procfs races are tolerated silently.
fn dangerous_task_state(pid: i32) -> Option<String> {
    let task_dir = format!("/proc/{}/task", pid);
    let entries = std::fs::read_dir(&task_dir).ok()?;
    for entry in entries.flatten() {
        let tid_path = entry.path();
        let tid = tid_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        if let Ok(w) = std::fs::read_to_string(tid_path.join("wchan")) {
            let w = w.trim();
            if !w.is_empty() && w != "0" && is_dangerous_wchan(w) {
                return Some(format!("tid {} in wchan {}", tid, w));
            }
        }
        if let Ok(sc) = std::fs::read_to_string(tid_path.join("syscall")) {
            let sc = sc.trim();
            if is_dangerous_syscall(sc) {
                return Some(format!("tid {} in syscall {}", tid, sc));
            }
        }
    }
    None
}

// Poll dangerous_task_state() with backoff. Returns after all threads clear
// the dangerous set, or when the total budget is exhausted (in which case we
// let libprofiler.a's own retry take over instead of blocking forever).
//
// Beyond avoiding the dangerous set, this also insists on attaching when the
// target's main thread is inside a GIL-releasing syscall (nanosleep, futex,
// poll, read/write, ...). A tight CUDA / bf16 tensorop loop can hold the GIL
// for the full duration of profiler_wait; if we attach mid-kernel-dispatch
// the injected clone thread will park on the GIL futex — glibc's pthread
// bookkeeping does not admit a new thread until the interpreter can drain
// it, and the whole run silently times out. Returns `true` when we found a
// safe window, `false` when the budget expired without ever seeing the main
// thread yield the GIL (caller should either retry, warn the user, or bail).
fn wait_for_safe_moment(pid: i32, budget_ms: u64) -> bool {
    let start = std::time::Instant::now();
    let mut delay_ms = 20u64;
    let mut last_reason_log = 0u64;
    loop {
        let elapsed_ms = start.elapsed().as_millis() as u64;
        match dangerous_task_state(pid) {
            None => {
                if main_thread_released_gil(pid) {
                    log::debug!(
                        "wait_for_safe_moment: main thread in GIL-releasing syscall, attaching now (elapsed {}ms)",
                        elapsed_ms
                    );
                    return true;
                }
                if elapsed_ms >= budget_ms {
                    log::warn!(
                        "wait_for_safe_moment: budget {}ms exhausted; main thread never yielded the GIL. \
                         Injection will likely stall — proceeding anyway so caller can decide.",
                        budget_ms
                    );
                    return false;
                }
                if elapsed_ms - last_reason_log >= 1000 {
                    log::warn!(
                        "wait_for_safe_moment: main thread holds GIL (CUDA-busy target?) — still waiting ({}ms/{}ms)",
                        elapsed_ms,
                        budget_ms
                    );
                    last_reason_log = elapsed_ms;
                }
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                delay_ms = std::cmp::min(delay_ms * 2, 200);
            }
            Some(reason) => {
                if elapsed_ms >= budget_ms {
                    log::warn!(
                        "wait_for_safe_moment: budget exhausted after {}ms ({}), proceeding anyway",
                        elapsed_ms,
                        reason
                    );
                    return false;
                }
                log::debug!(
                    "wait_for_safe_moment: {} — sleeping {}ms (elapsed {}ms/{})",
                    reason,
                    delay_ms,
                    elapsed_ms,
                    budget_ms
                );
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                delay_ms = std::cmp::min(delay_ms * 2, 200);
            }
        }
    }
}

#[derive(Debug)]
pub struct InjectHandle {
    handle: *mut injector::profiler_t,
    loose: bool,
    attached: bool,
}

impl Drop for InjectHandle {
    fn drop(&mut self) {
        if self.attached {
            if self.loose {
                if let Err(e) = self.retach() {
                    log::warn!("InjectHandle drop: retach failed: {}", e);
                }
            }
            if let Err(e) = self.detach() {
                log::warn!("InjectHandle drop: detach failed: {}", e);
            }
        }
    }
}

impl InjectHandle {
    pub fn init(pid: injector::pid_t, entry: Option<&str>) -> std::io::Result<Self> {
        let mut entry_name = match entry {
            Some(entry) => entry.to_string(),
            None => "entry".to_string(),
        };
        let handle = unsafe {
            let mut ptr: *mut injector::profiler_t = std::ptr::null_mut();
            let res = injector::profiler_init(&mut ptr, pid, entry_name.as_c_char_ptr_raw());
            match res {
                0 => Ok(ptr),
                _ => Err(Error::new(
                    ErrorKind::Other,
                    format!("profiler_init failed, err: {res}"),
                )),
            }
        }?;
        Ok(Self {
            handle,
            loose: false,
            attached: false,
        })
    }

    pub fn inject_library_file_in_clone_thread<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
    ) -> std::io::Result<()> {
        self.attach()?;
        self.inject_in_cloned_thread(path)?;
        self.loose()
    }

    pub fn direct_inject_library_file<P: AsRef<std::path::Path>>(
        &mut self,
        path: P,
    ) -> std::io::Result<*mut std::ffi::c_void> {
        self.attach()?;
        // Use profiler_inject instead of profiler_inject_in_cloned_thread
        // because profiler_inject handles absolute paths more reliably
        let handle = self.direct_inject(path)?;
        self.loose()?;
        Ok(handle)
    }

    fn direct_inject<P: AsRef<std::path::Path>>(
        &self,
        path: P,
    ) -> std::io::Result<*mut std::ffi::c_void> {
        let mut path_str = path
            .as_ref()
            .to_str()
            .ok_or(Error::new(
                ErrorKind::InvalidData,
                format!("invalid path to inject library {}", path.as_ref().display()),
            ))?
            .to_owned();
        let mut handle: *mut std::ffi::c_void = std::ptr::null_mut();
        let res = unsafe {
            injector::profiler_inject(self.handle, path_str.as_c_char_ptr_raw(), &mut handle)
        };
        match res {
            0 => {
                if handle.is_null() {
                    Err(Error::new(
                        ErrorKind::Other,
                        format!(
                            "profiler_inject returned null handle for path: {}",
                            path_str
                        ),
                    ))
                } else {
                    Ok(handle)
                }
            }
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_inject failed for path '{}', err: {res}", path_str),
            )),
        }
    }

    pub fn attach(&mut self) -> std::io::Result<()> {
        // NOTE: this lock is mandatory!! easy to trip over!!
        let _lock = INJECTOR_MUTEX.lock().unwrap();
        // Cap C-side wait4 loops (attach + inject_wait) so a blacklisted
        // userspace frame (e.g. __tls_get_addr) can't hang forever. 60s
        // covers full pyki loader lifecycle (start + profile + export ≈
        // 20-25s) while still bounding worst case. Parent wrapper has a
        // 90s wall-clock kill switch above this.
        unsafe { injector::profiler_set_timeout(self.handle, 60) };
        log::debug!("attach before: {:#?}", self.handle);
        let res = unsafe { injector::profiler_attach(self.handle) };
        match res {
            0 => {
                self.attached = true;
                Ok(())
            }
            _ => {
                // profiler_attach cleans up its own ptrace state internally
                // when it returns -2 (blacklist) or -1. Do NOT call
                // profiler_detach here — the C library has already freed its
                // internal state, and a second free crashes with double-free.
                // The caller must drop this handle and create a fresh one
                // for retry (see inject_pyki_loader / inject_cupti_loader).
                Err(Error::new(
                    ErrorKind::Other,
                    format!("profiler_attach failed, err: {res}"),
                ))
            }
        }
    }

    fn detach(&self) -> std::io::Result<()> {
        let res = unsafe { injector::profiler_detach(self.handle) };
        match res {
            0 => Ok(()),
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_detach failed, err: {res}"),
            )),
        }
    }

    fn inject_in_cloned_thread<P: AsRef<std::path::Path>>(&self, path: P) -> std::io::Result<()> {
        let mut path = path
            .as_ref()
            .to_str()
            .ok_or(Error::new(
                ErrorKind::InvalidData,
                format!("invalid path to inject library {}", path.as_ref().display()),
            ))?
            .to_owned();
        let res = unsafe {
            injector::profiler_inject_in_cloned_thread(
                self.handle,
                path.as_c_char_ptr_raw(),
                std::ptr::null_mut(),
            )
        };
        match res {
            0 => Ok(()),
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_inject_in_cloned_thread failed, err: {res}"),
            )),
        }
    }

    pub fn cupti_call_function(
        &self,
        handle: *mut std::ffi::c_void,
        func_name: &str,
    ) -> std::io::Result<()> {
        // Ensure the profiler is attached before calling the function.
        // Note: attach() is not called here since we may already be attached.
        // Callers should ensure attach happens before invocation.
        let func_name_cstr = std::ffi::CString::new(func_name).map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Invalid function name: {}", e),
            )
        })?;
        let res = unsafe { injector::profiler_call(self.handle, handle, func_name_cstr.as_ptr()) };
        match res {
            0 => Ok(()),
            _ => Err(Error::new(
                ErrorKind::Other,
                format!(
                    "profiler_call failed for function '{}', err: {res}",
                    func_name
                ),
            )),
        }
    }

    fn loose(&mut self) -> std::io::Result<()> {
        let res = unsafe { injector::profiler_loose(self.handle) };
        match res {
            0 => {
                self.loose = true;
                Ok(())
            }
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_loose failed, err: {res}"),
            )),
        }
    }

    pub fn _notify(&self, msg: u32) -> std::io::Result<()> {
        let res = unsafe { injector::profiler_notify(self.handle, msg as i32) };
        match res {
            0 => Ok(()),
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_notify failed, err: {res}"),
            )),
        }
    }

    pub fn wait(&self, msg: u32) -> std::io::Result<()> {
        let res = unsafe { injector::profiler_wait(self.handle, msg as i32) };
        match res {
            0 => Ok(()),
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_wait failed, err: {res}"),
            )),
        }
    }

    fn retach(&self) -> std::io::Result<()> {
        let res = unsafe { injector::profiler_retach(self.handle) };
        match res {
            0 => Ok(()),
            _ => Err(Error::new(
                ErrorKind::Other,
                format!("profiler_retach failed, err: {res}"),
            )),
        }
    }
}

pub fn inject_cupti_loader_static(pid: i32) -> Result<()> {
    // Check whether the library file has been copied to the target process's filesystem
    let library_path_in_target = format!("{}{}.so", r#const::CUPTI_DST_PATH, pid);
    let library_path_on_host = format!("/proc/{}/root{}", pid, library_path_in_target);

    if !Path::new(&library_path_on_host).exists() {
        return Err(ErrorCode::CuptiPluginError(Some(format!(
            "CUPTI library not found at {} for PID {}. Please ensure the library is copied first.",
            library_path_on_host, pid
        )))
        .into_error());
    }

    log::debug!(
        "CUPTI library found at {} for PID {}",
        library_path_on_host,
        pid
    );
    inject_cupti_loader(pid)?;
    Ok(())
}

pub fn inject_pyki_loader_static(python_code: String, pid: i32) -> Result<()> {
    let mut container_pid = pid;
    if utils::is_process_in_container(pid) {
        log::debug!("pid {} is in container", pid);
        container_pid = utils::convert_host_pid_to_container_pid(pid).unwrap();
        log::debug!("container pid is {}", container_pid);
    }

    let config_path = format!(
        "/proc/{}/root{}{}.txt",
        pid,
        r#const::LOADER_DST_PATH,
        container_pid
    );
    let config_file = Path::new(&config_path);
    log::debug!("config file path: {}", config_path);
    // Delete the file if it already exists
    if config_file.exists() {
        fs::remove_file(config_file)?;
    }

    fs::write(config_file, python_code)?;
    inject_pyki_loader(pid)?;
    Ok(())
}

fn inject_pyki_loader(pid: i32) -> Result<()> {
    // Bail out early if the target was poisoned by a prior failed injection —
    // libprofiler's clone thread would just queue behind the abandoned lock
    // and profiler_wait would age out at 60 s with no useful diagnostic.
    if let Some((addr, count)) = detect_orphan_dl_load_lock(pid) {
        log::error!(
            "inject_pyki_loader: pid {} has {} threads parked on futex {:#x} inside \
             ld-linux — glibc's _dl_load_lock has been orphaned by a prior failed \
             injection. Every subsequent dlopen in this target will deadlock. \
             Please restart the target process; refusing this injection to avoid \
             a 60 s profiler_wait timeout.",
            pid, count, addr
        );
        return Err(ErrorCode::PykiPluginError(Some(format!(
            "target pid {} has orphaned _dl_load_lock ({} waiters on {:#x}); \
             restart the target process to recover",
            pid, count, addr
        )))
        .into_error());
    }

    let mut retry_count = 10;
    let sleep_before_retry = 100000; // in microseconds
    let inject_path = format!("{}{}.so", r#const::LOADER_DST_PATH, pid);
    log::debug!("inject path: {:#?}", inject_path);
    while retry_count > 0 {
        // The injected clone thread parks on the GIL futex if we attach while
        // the target's main thread is mid-CUDA-kernel with the GIL held; the
        // interpreter can't admit the new thread and profiler_wait silently
        // ages out. Give a generous per-attempt budget (~20 s) so we're
        // patient enough to catch a syscall window even on a bf16 tensorop
        // tight loop; there are 10 retries above this, so worst case is
        // still bounded.
        let found_window = wait_for_safe_moment(pid, 20_000);
        if !found_window {
            log::warn!(
                "inject_pyki_loader: no GIL-release window found within 20s for pid {} \
                 (main thread is CUDA-busy?); attempting attach anyway — loader watchdog \
                 will surface any hang as LOADER_EXIT_GIL_TIMEOUT",
                pid
            );
        }
        let mut handle = InjectHandle::init(pid as injector::pid_t, Some("load_pyki"))?;
        log::debug!("init inject: {:#?}", handle);
        match handle.inject_library_file_in_clone_thread(Path::new(&inject_path)) {
            Ok(_) => {
                handle.wait(injector::THREAD_END)?;
                return Ok(());
            }
            Err(e) => {
                log::warn!("inject library error: {}, retry_count: {}", e, retry_count);
                // handle dropped here → detach cleans up any partial ptrace
                retry_count -= 1;
                std::thread::sleep(std::time::Duration::from_micros(sleep_before_retry));
            }
        }
    }
    Err(
        ErrorCode::PykiPluginError(Some(format!("Cannot attach Process {}", pid))).into_error(),
    )
}

fn inject_cupti_loader(pid: i32) -> Result<()> {
    let mut retry_count = 10;
    let sleep_before_retry = 100000; // in microseconds

    let inject_path = format!("{}{}.so", r#const::CUPTI_DST_PATH, pid);
    log::debug!(
        "inject path (from target process perspective): {}",
        inject_path
    );

    let library_path_on_host = format!("/proc/{}/root{}", pid, inject_path);
    if !Path::new(&library_path_on_host).exists() {
        return Err(ErrorCode::CuptiPluginError(Some(format!(
            "Library file does not exist in target process filesystem: {}",
            library_path_on_host
        )))
        .into_error());
    }

    // Same orphan check as inject_pyki_loader: cupti also goes through
    // dlopen inside the injected clone thread, so a poisoned _dl_load_lock
    // deadlocks it the exact same way.
    if let Some((addr, count)) = detect_orphan_dl_load_lock(pid) {
        log::error!(
            "inject_cupti_loader: pid {} has {} threads parked on futex {:#x} inside \
             ld-linux — glibc's _dl_load_lock has been orphaned by a prior failed \
             injection. Please restart the target process.",
            pid, count, addr
        );
        return Err(ErrorCode::CuptiPluginError(Some(format!(
            "target pid {} has orphaned _dl_load_lock ({} waiters on {:#x}); \
             restart the target process to recover",
            pid, count, addr
        )))
        .into_error());
    }

    let mut lib_handle: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut successful_handle: Option<InjectHandle> = None;
    while retry_count > 0 {
        // cupti path only needs dlopen; it never touches the interpreter, so
        // the GIL preference is irrelevant here. Ignore the return value.
        let _ = wait_for_safe_moment(pid, 500);
        let mut handle = InjectHandle::init(pid as injector::pid_t, Some("init"))?;
        log::debug!("init inject: {:#?}", handle);
        match handle.direct_inject_library_file(Path::new(&inject_path)) {
            Ok(injected_handle) => {
                if injected_handle.is_null() {
                    log::warn!(
                        "Injection returned null handle, retry_count: {}",
                        retry_count
                    );
                    retry_count -= 1;
                    if retry_count > 0 {
                        std::thread::sleep(std::time::Duration::from_micros(sleep_before_retry));
                    }
                    continue;
                }
                lib_handle = injected_handle;
                log::info!(
                    "Successfully injected CUPTI library into PID {}, handle: {:p}",
                    pid,
                    lib_handle
                );
                successful_handle = Some(handle);
                break;
            }
            Err(e) => {
                log::warn!("inject library error: {}, retry_count: {}", e, retry_count);
                retry_count -= 1;
                if retry_count > 0 {
                    std::thread::sleep(std::time::Duration::from_micros(sleep_before_retry));
                }
            }
        }
    }

    let mut handle = match successful_handle {
        Some(h) => h,
        None => {
            return Err(ErrorCode::CuptiPluginError(Some(format!(
                "Failed to inject library into Process {} after retries, or handle is null",
                pid
            )))
            .into_error());
        }
    };

    handle.wait(injector::THREAD_END)?;
    log::info!("CUPTI library injection completed for PID {}", pid);

    handle.attach()?;

    log::info!(
        "Calling InitializeInjection() in target process PID {} with handle: {:p}",
        pid,
        lib_handle
    );
    match handle.cupti_call_function(lib_handle, "InitializeInjection") {
        Ok(_) => {
            log::info!("InitializeInjection() called successfully for PID {}", pid);
        }
        Err(e) => {
            log::error!(
                "Failed to call InitializeInjection() for PID {}: {}",
                pid,
                e
            );
            return Err(ErrorCode::CuptiPluginError(Some(format!(
                "Failed to call InitializeInjection: {}",
                e
            )))
            .into_error());
        }
    }

    Ok(())
}
