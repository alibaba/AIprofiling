// src/plugins/pyki_plugin_wrapper.rs
use crate::collector::collector_name::CollectorName;
use crate::collector::event_handler::{EventHandler, SchedulerEvent};
use crate::collector::indicator_name::IndicatorName;
use crate::command::ProfileArgs;
use crate::error::ErrorCode;
use crate::meta::StepInfo;
use crate::plugins::plugin_adapter::{CollectorState, GenericPlugin, PluginConfig, PluginFunction};
use crate::r#const;
use crate::tools::chrome_time::ChromeTraceBaseTime;
use crate::tools::utils;
use anyhow::Result;
use async_trait::async_trait;
use glob::glob;
use log::debug;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader as StdBufReader};
use std::path::Path;
use std::process;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{Mutex, Semaphore};
use tracing as log;

// Global semaphore that caps concurrent file processing to avoid memory spikes
// from processing multiple large files at once.
static FILE_PROCESSING_SEMAPHORE: once_cell::sync::Lazy<Arc<Semaphore>> =
    once_cell::sync::Lazy::new(|| Arc::new(Semaphore::new(2))); // at most 2 files concurrently

/// Builder for Python execute() arguments.
pub struct PythonExecuteParams {
    params: HashMap<String, String>,
}

impl PythonExecuteParams {
    pub fn new() -> Self {
        Self {
            params: HashMap::new(),
        }
    }

    pub fn add_param(mut self, name: &str, value: &str) -> Self {
        self.params.insert(name.to_string(), value.to_string());
        self
    }

    pub fn add_bool_param(mut self, name: &str, value: bool) -> Self {
        self.params.insert(
            name.to_string(),
            if value {
                "True".to_string()
            } else {
                "False".to_string()
            },
        );
        self
    }

    pub fn add_optional_bool_param(mut self, name: &str, value: &Option<bool>) -> Self {
        if let Some(v) = value {
            self.params.insert(
                name.to_string(),
                if *v {
                    "True".to_string()
                } else {
                    "False".to_string()
                },
            );
        }
        self
    }

    pub fn add_number_param<T: ToString>(mut self, name: &str, value: T) -> Self {
        self.params.insert(name.to_string(), value.to_string());
        self
    }

    pub fn add_optional_number_param<T: ToString>(mut self, name: &str, value: &Option<T>) -> Self {
        if let Some(v) = value {
            self.params.insert(name.to_string(), v.to_string());
        }
        self
    }

    pub fn to_python_args(&self) -> String {
        let mut args = Vec::new();
        for (key, value) in &self.params {
            let formatted_value = if value == "True"
                || value == "False"
                || value == "on_start"
                || value == "on_stop"
                || value.parse::<f64>().is_ok()
            {
                value.clone()
            } else {
                format!("\"{}\"", value)
            };
            args.push(format!("{}={}", key, formatted_value));
        }
        args.join(",\n                         ")
    }
}

pub struct PykiPluginWrapper {
    config: PluginConfig,
    status: HashMap<i32, CollectorState>,
    stop_time: HashMap<i32, u128>,
    step_data: Arc<Mutex<HashMap<i32, (Vec<StepInfo>, Vec<i64>)>>>, // Arc<Mutex> so it can be shared across async tasks
}

impl PykiPluginWrapper {
    pub unsafe fn new(config: PluginConfig) -> Result<Self> {
        Ok(PykiPluginWrapper {
            config,
            status: HashMap::new(),
            stop_time: HashMap::new(),
            step_data: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn custom_init(&mut self, args: ProfileArgs) -> Result<()> {
        let output = if args.output.clone().unwrap() == "default" {
            format!("/proc/{}/root{}/", args.target_pid, r#const::DEFAULT_PATH)
        } else {
            args.output.clone().unwrap()
        };
        utils::clean_aiprof_files(&output)?;

        self.check_and_install_pyki(&args.target_pid.to_string())
            .await?;
        let binding = env::current_exe()?
            .parent()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
            + self.config.path.as_str();
        let src_path = binding.as_str();
        debug!("loader path is {}", src_path);

        utils::copy_file(
            src_path,
            &format!(
                "/proc/{}/root{}{}.so",
                args.target_pid,
                r#const::LOADER_DST_PATH,
                args.target_pid
            ),
            false,
        )?;

        self.status
            .insert(args.target_pid, CollectorState::InitSuccess);

        Ok(())
    }

    async fn custom_trigger(&mut self, args: ProfileArgs) -> Result<()> {
        let pid = args.target_pid.clone();

        if let Some(state) = self.status.get(&pid) {
            match state.cmp(&CollectorState::InitSuccess) {
                std::cmp::Ordering::Greater => {
                    log::info!(
                        "Pid: {} - PYKI has all ready been triggered, current state: {:?}",
                        pid,
                        state
                    );
                    return Ok(());
                }
                std::cmp::Ordering::Less => {
                    let err_msg = format!("Collector for PID {} was not initialized successfully, cannot trigger pyki, pyki state is: {:?}",
                        pid,
                        self.status.get(&pid)
                    );
                    log::error!("{}", err_msg);
                    return Err(ErrorCode::TriggerCollectorError(Some(err_msg)).into_error());
                }
                std::cmp::Ordering::Equal => {
                    // proceed with collection
                    self.status.insert(pid, CollectorState::Collecting);
                }
            }
        }

        let activities = if args
            .enable_indicator
            .iter()
            .any(|x| *x == IndicatorName::GPU)
        {
            if args
                .enable_indicator
                .iter()
                .any(|x| *x == IndicatorName::Torch)
            {
                "cpu,gpu"
            } else {
                // Should not reach this branch — kernel metrics should be collected by a different collector.
                log::error!(
                    "Pyki Collector enable gpu kernel indicator, but not enable torch indicator."
                );
                "cpu"
            }
        } else {
            ""
        };

        let enable_pystack = Some(
            args.enable_indicator
                .iter()
                .any(|x| *x == IndicatorName::PyStack),
        );

        let enable_snapshot = Some(
            args.enable_indicator
                .iter()
                .any(|x| *x == IndicatorName::Snapshot),
        );

        let params_builder = PythonExecuteParams::new()
            .add_param("path", r#const::DEFAULT_PATH)
            .add_param("prefix", &format!("{}{}", r#const::DEFAULT_PREFIX, pid))
            .add_bool_param("merge", false)
            .add_bool_param("compress", false)
            .add_param("activities", activities)
            .add_optional_bool_param("record_shapes", &args.record_shapes)
            .add_optional_bool_param("profile_memory", &args.profile_memory)
            .add_optional_bool_param("with_flops", &args.flops)
            .add_optional_bool_param("with_modules", &args.with_modules)
            .add_optional_bool_param("with_stack", &enable_pystack)
            .add_number_param("timeout", r#const::PYKI_DEFAULT_TIMEOUT)
            .add_number_param("start_timeout", r#const::PYKI_DEFAULT_TIMEOUT)
            .add_optional_number_param("duration", &Some(args.duration))
            .add_number_param("num_steps", &args.num_steps)
            .add_number_param("num_skip_steps", &args.num_skip_steps)
            .add_param("module", &args.iteration_module)
            .add_param("function", &args.iteration_function)
            .add_number_param("python_tracer_max_depth", r#const::PYTHON_TRACER_MAX_DEPTH)
            .add_number_param(
                "python_tracer_threshold_ns",
                r#const::INJECTOR_SLEEP_BEFORE_RETRY,
            )
            .add_bool_param("python_tracer_ignore_c_functions", false)
            .add_number_param("cpu_op_probability", 1.0)
            .add_optional_bool_param("with_torch_cuda_memory_trace", &enable_snapshot)
            .add_number_param(
                "torch_cuda_memory_trace_max_entry",
                &args.snapshot_max_entry.unwrap_or(1000000),
            )
            .add_bool_param("stop_on_cuda_oom", true)
            .add_param("on_profile_start", "on_start")
            .add_param("on_profile_stop", "on_stop");

        let python_args = params_builder.to_python_args();

        let output = args.output.clone().unwrap();

        log::debug!("python_args: {}", python_args);

        let step_data = Arc::clone(&self.step_data);
        // Wall-clock budget for the pyki-inject subprocess. libprofiler.a's
        // ptrace retry can silently stall against a target sitting on
        // __libc_malloc / __tls_get_addr for the whole session, which used to
        // leave the injector task blocked in child.wait() forever, no logs,
        // no error propagation. Cap the wait so the scheduler either observes
        // real success or moves on. Budget: profiling duration is capped at
        // u8 seconds; a large multiple still bounds worst case tightly.
        let duration_secs = args.duration as u64;
        let inject_budget = Duration::from_secs(std::cmp::max(duration_secs * 3, 60) + 30);
        // Injector body is 100% sync: std::process::Command + blocking
        // child.wait polling + stdout/stderr drain threads. Running it
        // under tokio::spawn (async) parks a worker for the whole
        // duration and, combined with the scheduler's std::sync::mpsc
        // recv, starved the runtime — auto_stop_timer never fired, no
        // pyki data was ever produced. spawn_blocking puts it on the
        // blocking pool where blocking is the whole point, and worker
        // threads stay free for the scheduler event loop and timers.
        let _ = tokio::task::spawn_blocking(move || {
            log::info!(
                "Pyki inject task started for PID {} (budget {:?})",
                pid,
                inject_budget
            );
            let current_exe = env::current_exe().unwrap_or_else(|_| {
                log::warn!(
                    "The current executable file path cannot be obtained. Use the default path"
                );
                Path::new("./CollectionFramework").to_path_buf()
            });

            let mut injector_handle = process::Command::new(&current_exe);
            injector_handle
                .arg("pyki-inject")
                .arg("--target-pid")
                .arg(&pid.to_string())
                .arg("--output")
                .arg(&output)
                .arg("--pyki-args")
                .arg(&python_args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            log::debug!("injector_handle: {:?}", injector_handle);

            let output = if args.output.clone().unwrap() == "default" {
                format!("/proc/{}/root{}/", pid, r#const::DEFAULT_PATH)
            } else {
                args.output.clone().unwrap()
            };
            let sender = EventHandler::global_sender();
            // Handle to the outer runtime so we can await the small async
            // bits (semaphore, sleep, post_processing_torch) from inside
            // this blocking thread. This is the standard tokio pattern.
            let rt = tokio::runtime::Handle::current();

            let mut retry_count = 0;

            while retry_count < 3 {
                let mut child = match injector_handle.spawn() {
                    Ok(child) => child,
                    Err(e) => {
                        let _ =
                            sender.send(SchedulerEvent::CollectFailed(CollectorName::Pyki, pid));
                        log::error!("The injection process(PID:{}) failed to start: {}", pid, e);
                        return;
                    }
                };
                let child_pid = child.id() as i32;
                log::debug!("child PID: {} -> injector PID: {}", child_pid, pid);

                // Drain stdout/stderr line-by-line into the CF log. Without
                // this the pipes fill up (~64KiB), pyki-inject blocks on
                // write, and looks identical to a hang.
                let stdout_thread = child.stdout.take().map(|out| {
                    std::thread::spawn(move || {
                        let reader = StdBufReader::new(out);
                        for line in reader.lines().flatten() {
                            log::info!(target: "pyki_inject", "[pid {}] {}", child_pid, line);
                        }
                    })
                });
                let stderr_thread = child.stderr.take().map(|err| {
                    std::thread::spawn(move || {
                        let reader = StdBufReader::new(err);
                        for line in reader.lines().flatten() {
                            log::warn!(target: "pyki_inject", "[pid {}] {}", child_pid, line);
                        }
                    })
                });

                // Wait with wall-clock cap. `try_wait` polls; a watchdog
                // thread wouldn't be able to unblock a sync `wait()`.
                let deadline = Instant::now() + inject_budget;
                let exit_status = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break Ok(status),
                        Ok(None) => {
                            if Instant::now() >= deadline {
                                log::error!(
                                    "pyki-inject (PID {}) exceeded budget {:?}, sending SIGKILL",
                                    child_pid,
                                    inject_budget
                                );
                                unsafe {
                                    libc::kill(child_pid, libc::SIGKILL);
                                }
                                // final wait so we reap the zombie
                                break child.wait().map(|s| s);
                            }
                            std::thread::sleep(Duration::from_millis(200));
                        }
                        Err(e) => break Err(e),
                    }
                };
                if let Some(t) = stdout_thread { let _ = t.join(); }
                if let Some(t) = stderr_thread { let _ = t.join(); }
                let exit_status = match exit_status {
                    Ok(status) => status,
                    Err(e) => {
                        let _ =
                            sender.send(SchedulerEvent::CollectFailed(CollectorName::Pyki, pid));
                        log::error!(
                            "An error occurred while waiting for the child process to end: {}",
                            e
                        );
                        return;
                    }
                };

                if exit_status.success() {
                    log::info!("Inject pyki successful for PID: {}", pid);
                    // 1s file-flush grace; blocking sleep is fine here —
                    // this whole function runs on the blocking pool.
                    std::thread::sleep(Duration::from_secs(1));

                    let input_pattern = format!(
                        "{}/{}{}*-torch-profile.json",
                        output,
                        r#const::DEFAULT_PREFIX,
                        pid
                    );

                    match glob(&input_pattern) {
                        Ok(entries) => {
                            let mut found_files = false;
                            for entry in entries {
                                match entry {
                                    Ok(path) => {
                                        found_files = true;

                                        let path_str = match path.to_str() {
                                            Some(p) => p.to_string(),
                                            None => {
                                                log::error!(
                                                    "Failed to convert path to string: {:?}",
                                                    path
                                                );
                                                let _ = sender.send(SchedulerEvent::CollectFailed(
                                                    CollectorName::Pyki,
                                                    pid,
                                                ));
                                                continue;
                                            }
                                        };
                                        let kernel_output = format!(
                                            "{}/{}{}-kernel-event.json",
                                            output,
                                            r#const::DEFAULT_PREFIX,
                                            pid
                                        );
                                        let step_data = Arc::clone(&step_data);
                                        let sender = sender.clone();
                                        // Hop back onto the runtime for the
                                        // async post-processing (tokio::fs +
                                        // Mutex + semaphore).
                                        rt.block_on(async move {
                                            let _permit =
                                                FILE_PROCESSING_SEMAPHORE.acquire().await.unwrap();
                                            let ts = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .expect("System time before Unix epoch!")
                                                .as_nanos();
                                            match post_processing_torch(
                                                pid,
                                                ts,
                                                &path_str,
                                                &kernel_output,
                                            )
                                            .await
                                            {
                                                Ok((stat_step, step_end_time)) => {
                                                    step_data
                                                        .lock()
                                                        .await
                                                        .insert(pid, (stat_step, step_end_time));
                                                    let _ = sender.send(
                                                        SchedulerEvent::WritingFinish(
                                                            CollectorName::Pyki,
                                                            pid,
                                                        ),
                                                    );
                                                }
                                                Err(e) => {
                                                    log::error!(
                                                        "Error processing profile file: {}",
                                                        e
                                                    );
                                                    let _ = sender.send(
                                                        SchedulerEvent::CollectFailed(
                                                            CollectorName::Pyki,
                                                            pid,
                                                        ),
                                                    );
                                                }
                                            }
                                        });
                                    }
                                    Err(e) => {
                                        log::error!("Error reading glob entry: {}", e);
                                        let _ = sender.send(SchedulerEvent::CollectFailed(
                                            CollectorName::Pyki,
                                            pid,
                                        ));
                                    }
                                }
                            }

                            if !found_files {
                                log::error!("No files matching pattern: {}", input_pattern);
                                let _ = sender
                                    .send(SchedulerEvent::WritingFinish(CollectorName::Pyki, pid));
                            }
                        }
                        Err(e) => {
                            log::error!(
                                "Failed to process glob pattern '{}': {}",
                                input_pattern,
                                e
                            );
                            let _ = sender
                                .send(SchedulerEvent::CollectFailed(CollectorName::Pyki, pid));
                        }
                    }
                    break;
                } else {
                    // Only injection failures are retried; capture the failing exit code.
                    match exit_status.code() {
                        Some(code) => {
                            log::error!(
                                "Injection execution failed. Exit code: {}, retry count: {}",
                                code,
                                retry_count
                            );
                        }
                        None => {
                            log::error!(
                                "The command was terminated by the signal, retry count: {}",
                                retry_count
                            );
                        }
                    }
                    std::thread::sleep(Duration::from_secs(2));
                    retry_count += 1;
                }
            }
            if retry_count >= 3 {
                log::error!("Injection failed after 3 attempts for PID: {}", pid);
                let _ = sender.send(SchedulerEvent::CollectFailed(CollectorName::Pyki, pid));
            }
        });
        Ok(())
    }

    async fn check_and_install_pyki(&mut self, pid: &str) -> Result<()> {
        let pid_num: i32 = pid.parse().unwrap_or(0);
        let pyexec = utils::get_python_executable(pid_num)?;
        let pip_version = utils::get_pip_version(&pyexec, pid)?;
        debug!("pip version is {}", pip_version);

        let binding = env::current_exe()?.parent().unwrap().join("pyki_dir");
        let exe_path = binding.to_string_lossy();
        utils::copy_file(
            &exe_path,
            format!("/proc/{}/root/pyki_dir", pid).as_str(),
            false,
        )?;
        if !utils::is_pyki_installed(&pyexec, pid)? {
            debug!("start pyki install for PID {}", pid);

            if !utils::is_pip_version_supported_for_break_system_packages(&pip_version)? {
                utils::install_pyki(&pyexec, false, pid)?;
            } else {
                utils::install_pyki(&pyexec, true, pid)?;
            }

            if !utils::is_pyki_installed(&pyexec, pid)? {
                log::error!("pyki install failed !");
                return Err(ErrorCode::PykiInstallFailed(None).into_error());
            }
        } else {
            debug!("start pyki reinstall for PID {}", pid);
            let curr_pyki_version = utils::get_installed_pyki_version(&pyexec, pid)?;
            let release_pyki_version = utils::get_release_pyki_version()?;
            debug!(
                "release_pyki_version: {}, curr_pyki_version: {}",
                release_pyki_version, curr_pyki_version
            );
            if release_pyki_version != curr_pyki_version {
                // NOTE: reinstall whenever the versions differ
                if !utils::is_pip_version_supported_for_break_system_packages(&pip_version)? {
                    utils::uninstall_pyki(&pyexec, false, pid)?;
                    if !utils::is_pyki_installed(&pyexec, pid)? {
                        debug!(
                            "uninstall pyki ok, start install version {} ",
                            release_pyki_version
                        );
                        utils::install_pyki(&pyexec, false, pid)?;
                        if !utils::is_pyki_installed(&pyexec, pid)? {
                            debug!("pyki install failed, upgrade fail");
                            return Err(ErrorCode::PykiInstallFailed(None).into_error());
                        }
                    }
                } else {
                    utils::uninstall_pyki(&pyexec, true, pid)?;
                    if !utils::is_pyki_installed(&pyexec, pid)? {
                        debug!(
                            "uninstall pyki ok, start install version {}",
                            release_pyki_version
                        );
                        utils::install_pyki(&pyexec, true, pid)?;
                        if !utils::is_pyki_installed(&pyexec, pid)? {
                            debug!("pyki install failed, upgrade fail");
                            return Err(ErrorCode::PykiInstallFailed(None).into_error());
                        }
                    }
                }
            }
        }
        // If the directory exists, remove it entirely.
        let pyki_dir_path = format!("/proc/{}/root/pyki_dir", pid);
        if Path::new(&pyki_dir_path).exists() {
            let _ = fs::remove_dir_all(&pyki_dir_path).await;
        }
        Ok(())
    }

    async fn custom_shutdown(&mut self, pid: i32) {
        self.status.insert(pid, CollectorState::ShuttingDown);
    }
}

impl Drop for PykiPluginWrapper {
    fn drop(&mut self) {
        log::debug!("Dropping PykiPluginWrapper, cleaning up resources...");

        // Mirror CUPTIPluginWrapper::drop: do NOT delete the libprofiler.so
        // injected into the target. The target holds a file-backed mmap from
        // dlopen; removing the underlying file while the target is still
        // running causes later page-ins to raise SIGBUS and kill the user
        // process. Only clean up small files like sockets/txt; the .so stays
        // in the target's /tmp and is reclaimed by the OS once the process
        // exits.
        for (pid, _) in &self.status {
            let container_pid = utils::convert_host_pid_to_container_pid(*pid).unwrap_or(0);

            let cf_sock_path = format!("/proc/{}/root{}{}", pid, r#const::CF_UNIXSOCK, pid);
            if let Ok(_) = std::fs::remove_file(&cf_sock_path) {
                log::debug!("Removed Pyki socket file: {}", cf_sock_path);
            }

            let cf_txt_path = format!(
                "/proc/{}/root{}{}.txt",
                pid,
                r#const::LOADER_DST_PATH,
                container_pid
            );
            if let Ok(_) = std::fs::remove_file(&cf_txt_path) {
                log::debug!("Removed Pyki txt file: {}", cf_txt_path);
            }
        }

        log::debug!("PykiPluginWrapper resources cleanup completed");
    }
}

#[async_trait]
impl GenericPlugin for PykiPluginWrapper {
    async fn call_function(&mut self, func: PluginFunction, params: &Value) -> Result<Value> {
        match func {
            PluginFunction::Init => {
                let args: ProfileArgs = serde_json::from_value(params.clone())?;
                self.custom_init(args).await?;
                Ok(Value::Null)
            }
            PluginFunction::Trigger => {
                let args: ProfileArgs = serde_json::from_value(params.clone())?;
                self.custom_trigger(args).await?;
                Ok(Value::Null)
            }
            PluginFunction::Stop => {
                let pid = params.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("System time before Unix epoch!")
                    .as_nanos()
                    + 1000000000; // add 1 second to ensure all events are captured
                self.stop_time.insert(pid, ts);
                Ok(Value::Null)
            }
            PluginFunction::Shutdown => {
                let pid: i32 = serde_json::from_value(params.clone())?;
                debug!("Pyki Plugin call Shutdown: {}", pid);
                self.custom_shutdown(pid).await;

                // Return step data
                let step_data = self.step_data.lock().await.get(&pid).cloned();
                Ok(serde_json::to_value(step_data)?)
            }
        }
    }
}

async fn post_processing_torch(
    pid: i32,
    stop_time: u128,
    torch_output: &str,
    kernel_output: &str,
) -> Result<(Vec<StepInfo>, Vec<i64>)> {
    // 1. Stream through a temp file to avoid loading the whole JSON into memory.
    let temp_output = format!("{}.tmp", torch_output);

    // 2. Open the input file and wrap it in a buffered reader.
    let input_file = fs::File::open(torch_output).await?;
    let reader = BufReader::with_capacity(1024 * 1024, input_file); // 1MB buffer

    // 3. Create the buffered writer for the output file.
    let output_file = fs::File::create(&temp_output).await?;
    let mut output_writer = BufWriter::with_capacity(1024 * 1024, output_file); // 1MB

    let kernel_file = fs::File::create(kernel_output).await?;
    let mut kernel_writer = BufWriter::with_capacity(1024 * 1024, kernel_file); // 256KB

    let relative_timebase = ChromeTraceBaseTime::get_base_time() / 1000;
    let stop_time = (stop_time / 1000 - relative_timebase as u128) as i64;

    // Accumulates per-step statistics
    let mut stat_step: Vec<StepInfo> = Vec::new();
    let mut step_end_time: Vec<i64> = Vec::new();
    let iteration_step_regex = Regex::new(r"^IterationStep.*")?;

    // 4. Stream through the JSON line by line.
    let mut lines = reader.lines();
    let mut is_first_event = true;
    let mut kernel_is_first = true;
    let mut in_trace_events = false;
    let mut brace_count = 0;
    let mut event_buffer = String::with_capacity(4096);

    // Write the opening bracket of the JSON array.
    output_writer.write_all(b"[").await?;
    kernel_writer.write_all(b"[").await?;

    while let Some(line) = lines.next_line().await? {
        let trimmed = line.trim();

        // Detect the start of the traceEvents array.
        if trimmed.contains("\"traceEvents\"") {
            in_trace_events = true;
            continue;
        }

        if !in_trace_events {
            continue;
        }

        // Track braces to identify a complete event object.
        for ch in trimmed.chars() {
            if ch == '{' {
                if brace_count == 0 {
                    event_buffer.clear();
                }
                brace_count += 1;
                event_buffer.push(ch);
            } else if ch == '}' {
                event_buffer.push(ch);
                brace_count -= 1;

                // Found a complete event object.
                if brace_count == 0 {
                    // Parse and process a single event.
                    if let Ok(mut event) = serde_json::from_str::<Value>(&event_buffer) {
                        // Check for the "Record Window End" event (the last event).
                        if event
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map_or(false, |s| s == "Record Window End")
                        {
                            event_buffer.clear();
                            continue;
                        }

                        // Read timestamp and duration.
                        let ts_raw = event.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let dur_raw = event.get("dur").and_then(|v| v.as_f64()).unwrap_or(0.0);

                        let ts = ts_raw as i64;
                        let dur = dur_raw as i64;

                        // Adjust the timestamp.
                        let adjusted_ts = if ts > relative_timebase {
                            let mut new_ts = ts - relative_timebase;
                            if new_ts > stop_time {
                                new_ts = stop_time;
                            }
                            new_ts
                        } else {
                            if ts > stop_time {
                                stop_time
                            } else {
                                ts
                            }
                        };

                        // Update the timestamp in the event object.
                        if let Some(obj) = event.as_object_mut() {
                            obj.insert("ts".to_string(), Value::Number(adjusted_ts.into()));
                        }

                        // Check for a user_annotation IterationStep event.
                        let cat = event.get("cat").and_then(|c| c.as_str()).unwrap_or("");
                        let name = event.get("name").and_then(|n| n.as_str()).unwrap_or("");

                        if cat == "user_annotation" && iteration_step_regex.is_match(name) {
                            // Record step info.
                            stat_step.push(StepInfo {
                                id: name.to_string(),
                                start: adjusted_ts,
                                dur,
                                loss: "N/A".to_string(), // loss info not available yet
                            });

                            // Record the step's end time.
                            if step_end_time.is_empty() {
                                step_end_time.push(adjusted_ts);
                            }
                            step_end_time.push(adjusted_ts + dur);
                        }

                        // Check whether this is a kernel event.
                        let is_kernel = cat == "kernel";

                        // Write to the main output file.
                        if !is_first_event {
                            output_writer.write_all(b",").await?;
                        }
                        is_first_event = false;

                        let event_json = serde_json::to_string(&event)?;
                        output_writer.write_all(event_json.as_bytes()).await?;

                        // Also mirror kernel events into the kernel file.
                        if is_kernel {
                            if let Some(obj) = event.as_object_mut() {
                                obj.insert("pid".to_string(), Value::Number(pid.into()));
                            }

                            if !kernel_is_first {
                                kernel_writer.write_all(b",").await?;
                            }
                            kernel_is_first = false;

                            let kernel_json = serde_json::to_string(&event)?;
                            kernel_writer.write_all(kernel_json.as_bytes()).await?;
                        }
                    }
                    event_buffer.clear();
                }
            } else if brace_count > 0 {
                event_buffer.push(ch);
            }
        }

        // Detect the end of the traceEvents array.
        if trimmed.starts_with("]") && in_trace_events {
            break;
        }
    }

    // Write the closing bracket of the JSON array.
    output_writer.write_all(b"]").await?;
    kernel_writer.write_all(b"]").await?;

    // 5. Flush buffers.
    output_writer.flush().await?;
    kernel_writer.flush().await?;

    // 6. Close file handles.
    drop(output_writer);
    drop(kernel_writer);

    // 7. Atomically replace the original file.
    fs::rename(&temp_output, torch_output).await?;

    // 8. Return the collected step statistics.
    Ok((stat_step, step_end_time))
}
