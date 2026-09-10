use crate::collector::collector_name::CollectorName;
use crate::collector::event_handler::{EventHandler, SchedulerEvent};
use crate::command::ProfileArgs;
use crate::error::ErrorCode;
use crate::plugins::plugin_adapter::{CollectorState, GenericPlugin, PluginConfig, PluginFunction};
use crate::r#const;
use crate::tools::utils;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::process;
use std::time::SystemTime;
use tokio::fs;
use tracing as log;

pub struct CUPTIPluginWrapper {
    config: PluginConfig,
    status: HashMap<i32, CollectorState>,
    // For each PID: (host_output_path, local_output_path, trigger_start) —
    // trigger_start is the wall-clock instant we scheduled this collection.
    // On shutdown we only copy the target-side file if its mtime post-dates
    // trigger_start, otherwise it is a stale file from an earlier run left
    // behind by libcuprof.so's persistent (nodelete) load in the target.
    output_paths: HashMap<i32, (String, String, SystemTime)>,
}

// KEY=VALUE configuration fields for cuprof (see src/plugins/cuprof/docs/embedding.md).
struct CuprofConfig {
    output: String,
    duration_sec: u32,
    verbose: bool,
    socket_path: String,
}

impl CUPTIPluginWrapper {
    pub unsafe fn new(config: PluginConfig) -> Result<Self> {
        Ok(CUPTIPluginWrapper {
            config,
            status: HashMap::new(),
            output_paths: HashMap::new(),
        })
    }

    async fn custom_init(&mut self, args: ProfileArgs) -> Result<()> {
        // Try multiple candidate library paths (prefer the one next to the executable).
        let mut possible_paths = Vec::new();

        // 1. libcuprof.so in the current directory (highest priority).
        possible_paths.push("./libcuprof.so".to_string());

        // 2. Path relative to the executable (the one configured in config.yaml).
        if let Ok(exe_path) = env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                if let Some(parent_str) = parent.to_str() {
                    possible_paths.push(format!("{}{}", parent_str, self.config.path.as_str()));
                }
            }
        }

        // 3. Other path variants under the current directory.
        possible_paths.push(format!("./{}", self.config.path.trim_start_matches('/')));
        possible_paths.push(self.config.path.trim_start_matches('/').to_string());

        // 4. Absolute path (if config.path is already absolute).
        if self.config.path.starts_with('/') {
            possible_paths.push(self.config.path.clone());
        }

        // Pick the first existing file.
        let mut src_path: Option<String> = None;
        for path in &possible_paths {
            log::debug!("Checking cuprof library path: {}", path);
            if Path::new(path).exists() {
                src_path = Some(path.clone());
                log::info!("Found cuprof library at: {}", path);
                break;
            }
        }

        let src_path = src_path.ok_or_else(|| {
            ErrorCode::CuptiPluginError(Some(format!(
                "cuprof library not found in any of these paths: {:?}",
                possible_paths
            )))
            .into_error()
        })?;

        // Copy the library into the target process's filesystem.
        let dst_path = format!(
            "/proc/{}/root{}{}.so",
            args.target_pid,
            r#const::CUPTI_DST_PATH,
            args.target_pid
        );
        utils::copy_file(&src_path, &dst_path, false)
            .context("Failed to copy cuprof library to target process")?;

        // libcuprof.so depends on libcupti.so.<major>, but it is dlopen'd by the
        // target's dynamic linker after ptrace injection — resolution happens in
        // the target's address space using its own search paths, so the
        // injector-side LD_LIBRARY_PATH does not apply. The target's CUPTI
        // version may also mismatch the one used at build time (e.g. host runs
        // CUDA 13 while libcuprof.so was linked against 12). libcuprof.so's
        // RPATH carries $ORIGIN, i.e. /tmp inside the target namespace, so
        // dropping a vendored libcupti with the matching soname into the
        // target's /tmp is enough to make dlopen succeed.
        if let Err(e) = self.stage_cupti_runtime(&src_path, args.target_pid) {
            log::warn!(
                "Failed to stage vendored libcupti next to cuprof for PID {}: {}. \
                 Injection will rely on the target's own CUPTI being discoverable.",
                args.target_pid,
                e
            );
        }

        self.status
            .insert(args.target_pid, CollectorState::InitSuccess);

        log::info!(
            "cuprof library copied successfully for PID: {}",
            args.target_pid
        );
        Ok(())
    }

    // Stage a vendored libcupti whose soname matches libcuprof.so's requirement
    // into the target process's /tmp, so it sits next to $ORIGIN when dlopen
    // runs after injection.
    // Selection strategy: read libcuprof.so's DT_NEEDED, find the entry of the
    // form libcupti.so.<major>, then copy the vendored candidate whose soname
    // matches exactly.
    fn stage_cupti_runtime(&self, cuprof_src: &str, target_pid: i32) -> Result<()> {
        let needed = utils::read_needed_soname(cuprof_src, "libcupti.so.")
            .context("cannot determine libcupti soname required by libcuprof.so")?;

        // The vendored cupti directory sits next to libcuprof.so under
        // /opt/aiprof (the Dockerfile COPYs it to /opt/aiprof/cupti/).
        // config.path points at libcuprof.so itself, so derive the cupti/
        // subdirectory from its parent.
        let cuprof_dir = Path::new(cuprof_src)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| Path::new(".").to_path_buf());
        let mut search_dirs = vec![cuprof_dir.join("cupti"), cuprof_dir.clone()];
        if let Ok(exe_path) = env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                search_dirs.push(parent.join("cupti"));
                search_dirs.push(parent.to_path_buf());
            }
        }

        let mut chosen: Option<std::path::PathBuf> = None;
        for dir in &search_dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !name.starts_with("libcupti.so.") {
                        continue;
                    }
                    if let Ok(soname) = utils::read_soname(path.to_string_lossy().as_ref()) {
                        if soname == needed {
                            chosen = Some(path);
                            break;
                        }
                    }
                }
            }
            if chosen.is_some() {
                break;
            }
        }

        let chosen = chosen.ok_or_else(|| {
            ErrorCode::CuptiPluginError(Some(format!(
                "no vendored libcupti with soname '{}' found in {:?}",
                needed, search_dirs
            )))
            .into_error()
        })?;

        let dst = format!("/proc/{}/root/tmp/{}", target_pid, needed);
        utils::copy_file(chosen.to_string_lossy().as_ref(), &dst, false)
            .with_context(|| format!("failed to stage {} for target", needed))?;
        log::info!(
            "Staged vendored CUPTI {} -> {} (soname {})",
            chosen.display(),
            dst,
            needed
        );
        Ok(())
    }

    async fn custom_trigger(&mut self, args: ProfileArgs) -> Result<()> {
        let pid = args.target_pid;

        // Check status.
        if let Some(state) = self.status.get(&pid) {
            match state.cmp(&CollectorState::InitSuccess) {
                std::cmp::Ordering::Greater => {
                    log::info!(
                        "PID: {} - cuprof has already been triggered, current state: {:?}",
                        pid,
                        state
                    );
                    return Ok(());
                }
                std::cmp::Ordering::Less => {
                    let err_msg = format!(
                        "Collector for PID {} was not initialized successfully, cannot trigger cuprof, state is: {:?}",
                        pid,
                        self.status.get(&pid)
                    );
                    log::error!("{}", err_msg);
                    return Err(ErrorCode::TriggerCollectorError(Some(err_msg)).into_error());
                }
                std::cmp::Ordering::Equal => {
                    self.status.insert(pid, CollectorState::Collecting);
                }
            }
        } else {
            return Err(ErrorCode::TriggerCollectorError(Some(format!(
                "PID {} not found in status map",
                pid
            )))
            .into_error());
        }

        let mut output = args.output.clone().unwrap_or_else(|| ".".to_string());
        let container_pid = if utils::is_process_in_container(pid) {
            utils::convert_host_pid_to_container_pid(pid).unwrap_or(pid)
        } else {
            pid
        };
        if output == "default" {
            output = format!("/proc/{}/root{}/", container_pid, r#const::DEFAULT_PATH,);
        }

        // cuprof writes from inside the target process in the target's filesystem
        // namespace. If we're in a container with --pid=host, the container's /tmp
        // is NOT the host's /tmp. We write cuprof output to the target's /tmp,
        // then copy the file back to our workdir after collection finishes.
        let target_output_name = format!("{}{}_cupti.json", r#const::DEFAULT_PREFIX, pid);
        let target_output_path = format!("{}/{}", r#const::DEFAULT_PATH, target_output_name);
        let host_output_path = format!("/proc/{}/root{}", pid, target_output_path);
        let local_output_path = format!("{}/{}", output, target_output_name);

        // Output filename kept in sync with the pyki side
        // (AIProf_<pid>_cupti.json); dashboardServer's findTraceFile depends on
        // this prefix.
        // NOTE: `target_output_path` is the path as seen by the target process
        // (e.g. /tmp/AIProf_179145_cupti.json) — that's what goes into the cfg.
        let log_file = target_output_path.clone();
        // Lifecycle message socket between cuprof and CF; the three message
        // names (CUPTIProfilingStart/Stop/WriterOver) are kept aligned with
        // cuprof in const.rs.
        let cf_socket_path = format!("{}{}", r#const::CF_UNIXSOCK, pid);

        let cuprof_cfg = CuprofConfig {
            output: log_file.clone(),
            duration_sec: args.duration as u32,
            verbose: false,
            socket_path: cf_socket_path,
        };

        // Perform injection and config write in a background task.
        // Remove any stale trace file left in the target by a previous run:
        // libcuprof.so stays mapped (linker flag `-z nodelete`) and holds the
        // last write path, so an old ${DEFAULT_PATH}/AIProf_<pid>_cupti.json
        // can sit there for hours. If InitializeInjection then fails, we must
        // not fall back to that stale file and report it as this task's output.
        let stale_path = format!("/proc/{}/root{}", pid, target_output_path);
        if Path::new(&stale_path).exists() {
            match std::fs::remove_file(&stale_path) {
                Ok(_) => log::info!("Removed stale cuprof trace before injection: {}", stale_path),
                Err(e) => log::warn!("Failed to remove stale cuprof trace {}: {}", stale_path, e),
            }
        }
        let trigger_start = SystemTime::now();
        self.output_paths
            .insert(pid, (host_output_path, local_output_path, trigger_start));
        // See pyki_plugin_wrapper.rs custom_trigger for the reasoning:
        // the injector body is fully sync (child.wait()) and would starve
        // tokio workers if run under tokio::spawn. Blocking pool is the
        // right home for it.
        let _ = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            // Config write is async (tokio::fs); hop back onto the runtime
            // for it.
            let cfg_result = rt.block_on(async {
                Self::write_cuprof_config(pid, container_pid, &cuprof_cfg).await
            });
            if let Err(e) = cfg_result {
                let sender = EventHandler::global_sender();
                log::error!(
                    "Failed to write cuprof config for PID {} before injection: {}",
                    pid,
                    e
                );
                let _ = sender.send(SchedulerEvent::CollectFailed(CollectorName::CUPTI, pid));
                return;
            }
            log::info!(
                "cuprof config written for PID: {} (container pid: {})",
                pid,
                container_pid
            );

            let current_exe = env::current_exe().unwrap_or_else(|_| {
                log::warn!(
                    "The current executable file path cannot be obtained. Use the default path"
                );
                Path::new("./CollectionFramework").to_path_buf()
            });

            // Inject the cuprof library (the config file has already been written,
            // so InitializeInjection() can read it).
            let mut injector_handle = process::Command::new(&current_exe);
            injector_handle
                .arg("cupti-inject")
                .arg("--target-pid")
                .arg(&pid.to_string())
                .arg("--output")
                .arg(&output);
            log::debug!(
                "Injecting cuprof library for PID: {:?}, output: {}",
                injector_handle,
                output
            );

            let sender = EventHandler::global_sender();

            let mut retry_count = 0;

            while retry_count < 3 {
                let mut child = match injector_handle.spawn() {
                    Ok(child) => child,
                    Err(e) => {
                        let _ =
                            sender.send(SchedulerEvent::CollectFailed(CollectorName::CUPTI, pid));
                        log::error!("The injection process(PID:{}) failed to start: {}", pid, e);
                        return;
                    }
                };

                log::debug!("child PID: {} -> injector PID: {}", child.id(), pid);

                // Wait for injection to finish.
                let exit_status = match child.wait() {
                    Ok(status) => status,
                    Err(e) => {
                        let _ =
                            sender.send(SchedulerEvent::CollectFailed(CollectorName::CUPTI, pid));
                        log::error!(
                            "An error occurred while waiting for the injection process to end: {}",
                            e
                        );
                        return;
                    }
                };
                if exit_status.success() {
                    log::info!("cuprof library injected successfully for PID: {}", pid);
                    break;
                } else {
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
                    retry_count += 1;
                }
            }

            if retry_count >= 3 {
                log::error!("Injection failed after 3 attempts for PID: {}", pid);
                let _ = sender.send(SchedulerEvent::CollectFailed(CollectorName::CUPTI, pid));
            }
        });

        // self.handler.insert(pid, handler);
        Ok(())
    }

    async fn write_cuprof_config(pid: i32, container_pid: i32, config: &CuprofConfig) -> Result<()> {
        // cuprof's LoadCfgFile tries $CUPROF_CONFIG, then
        // /tmp/cuprof_<pid>.cfg (<pid> is the namespace-local pid), then
        // /tmp/cuprof.cfg. We write the second one directly through the
        // target's rootfs to avoid relying on the injector propagating
        // environment variables.
        let config_path = format!(
            "/proc/{}/root/tmp/cuprof_{}.cfg",
            pid, container_pid
        );

        // Ensure the directory exists.
        let dir_path = Path::new(&config_path).parent().unwrap();
        fs::create_dir_all(dir_path).await?;

        // Build the KEY=VALUE config (see src/plugins/cuprof/docs/embedding.md).
        let mut config_content = String::new();
        config_content.push_str(&format!("CUPROF_OUTPUT={}\n", config.output));
        if config.duration_sec > 0 {
            config_content.push_str(&format!("CUPROF_DURATION={}\n", config.duration_sec));
        }
        if config.verbose {
            config_content.push_str("CUPROF_VERBOSE=1\n");
        }
        if !config.socket_path.is_empty() {
            config_content.push_str(&format!("CUPROF_SOCKET={}\n", config.socket_path));
        }

        fs::write(&config_path, config_content)
            .await
            .with_context(|| format!("Failed to write cuprof config to: {}", config_path))?;

        log::debug!("cuprof config written to: {}", config_path);
        Ok(())
    }

    async fn custom_shutdown(&mut self, pid: i32) {
        self.status.insert(pid, CollectorState::ShuttingDown);

        // Copy the cuprof trace from the target's namespace into the container-
        // local workdir so the uploader picks it up. This runs after the
        // scheduler's StopCollector phase, before packaging.
        //
        // Skip stale files: libcuprof.so stays mapped in the target (`-z
        // nodelete`), so a prior successful run leaves a trace at the same
        // path. When InitializeInjection() fails or cuprof never actually
        // wrote, mtime will be earlier than this task's trigger_start and we
        // must not report that old file as the current task's output.
        if let Some((host_path, local_path, trigger_start)) = self.output_paths.remove(&pid) {
            let mtime = std::fs::metadata(&host_path).and_then(|m| m.modified());
            match mtime {
                Ok(mt) if mt >= trigger_start => match std::fs::copy(&host_path, &local_path) {
                    Ok(bytes) => log::info!(
                        "cuprof trace copied ({} bytes): {} -> {}",
                        bytes,
                        host_path,
                        local_path
                    ),
                    Err(e) => log::warn!(
                        "Failed to copy cuprof trace from {} to {}: {}",
                        host_path,
                        local_path,
                        e
                    ),
                },
                Ok(mt) => {
                    let age_s = trigger_start
                        .duration_since(mt)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    log::warn!(
                        "Skipping stale cuprof trace {} (mtime is {}s older than trigger start; cuprof likely failed to run this task)",
                        host_path,
                        age_s
                    );
                }
                Err(e) => {
                    log::warn!(
                        "cuprof trace not present at {} (cuprof produced no output for this task): {}",
                        host_path,
                        e
                    );
                }
            }
        }

        log::debug!("shutdown cuprof: {:#?}", self.status);
    }
}

impl Drop for CUPTIPluginWrapper {
    fn drop(&mut self) {
        log::debug!("Dropping CuptiPluginWrapper, cleaning up resources...");

        // Clean up socket and config files, but do NOT delete
        // libcuprof.so / libcupti.so: the target's dlopen holds an mmap
        // on them, and removing the .so while the target is alive causes
        // later kernel page-ins to raise SIGBUS and kill the target.
        // These files under /tmp are small (<1MB); once the target exits,
        // /proc/<pid>/root is gone and the files are cleaned up naturally
        // (or left in the target's /tmp for the OS to reclaim).
        for (pid, _) in &self.status {
            let cf_sock_path = format!("/proc/{}/root{}{}", pid, r#const::CF_UNIXSOCK, pid);
            if let Ok(_) = std::fs::remove_file(&cf_sock_path) {
                log::debug!("Removed cuprof socket file: {}", cf_sock_path);
            }

            let config_path = format!("/proc/{}/root/tmp/cuprof_{}.cfg", pid, pid);
            if let Ok(_) = std::fs::remove_file(&config_path) {
                log::debug!("Removed cuprof config file: {}", config_path);
            }
        }

        log::debug!("CuptiPluginWrapper resources cleanup completed");
    }
}

#[async_trait]
impl GenericPlugin for CUPTIPluginWrapper {
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
            PluginFunction::Stop => Ok(Value::Null),
            PluginFunction::Shutdown => {
                let pid: i32 = serde_json::from_value(params.clone())?;
                log::debug!("CUPTI Plugin call Shutdown: {}", pid);
                self.custom_shutdown(pid).await;
                Ok(Value::Null)
            }
        }
    }
}
