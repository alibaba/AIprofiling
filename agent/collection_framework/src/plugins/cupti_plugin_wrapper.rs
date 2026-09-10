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
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Mutex, OnceLock};
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

/// Which of several vendored files that share one SONAME should be staged.
///
/// There is no universally correct answer. CUPTI's ABI is fixed within a SONAME
/// family, but its *minimum driver requirement* rises with each patch level, so
/// the newest release covers the widest set of GPU architectures while the
/// oldest one starts on the oldest drivers. Which of the two matters depends on
/// the fleet, so the default is overridable at runtime rather than only by
/// rebuilding the client image.
#[derive(Debug, PartialEq, Eq)]
enum CuptiPreference {
    Highest,
    Lowest,
    Exact(String),
}

/// Parse `AIPROF_CUPTI_PREFER`. Split out from reading the environment so the
/// fallbacks are testable without mutating process state.
fn parse_cupti_preference(raw: Option<&str>) -> CuptiPreference {
    match raw.unwrap_or("").trim() {
        "" | "highest" => CuptiPreference::Highest,
        "lowest" => CuptiPreference::Lowest,
        name if name.starts_with("libcupti.so.") => CuptiPreference::Exact(name.to_string()),
        other => {
            log::warn!(
                "AIPROF_CUPTI_PREFER='{}' is neither 'highest', 'lowest' nor a \
                 libcupti.so.<version> file name; staging the highest release",
                other
            );
            CuptiPreference::Highest
        }
    }
}

fn cupti_preference() -> CuptiPreference {
    parse_cupti_preference(env::var("AIPROF_CUPTI_PREFER").ok().as_deref())
}

/// Rank a vendored `libcupti.so.<version>` file name: compare the dotted version
/// component by component as numbers, so `2025.2.1` outranks `2024.3.2` and
/// `10.2.75` outranks `9.0.176`. A plain string compare gets both wrong.
///
/// `None` means "not a release name". NVIDIA's convention is
/// `libcupti.so.<major>.<minor>.<patch>`; a hand-made `libcupti.so.12` — the
/// SONAME rather than a release — would compare as `[12]` and lose to
/// `[2023, 2, 1]`, silently staging an older CUPTI, so anything with fewer than
/// three numeric components is ranked below every conforming name instead.
fn cupti_version_key(file_name: &str) -> Option<Vec<u64>> {
    let components: Vec<&str> = file_name
        .trim_start_matches("libcupti.so.")
        .split('.')
        .collect();
    if components.len() < 3 {
        return None;
    }
    components.iter().map(|c| c.parse::<u64>().ok()).collect()
}

/// Total order over candidate file names. The name is the final tie-break, so
/// the result never depends on the order `read_dir` happened to yield.
fn compare_cupti_candidates(a: &str, b: &str) -> std::cmp::Ordering {
    cupti_version_key(a)
        .cmp(&cupti_version_key(b))
        .then_with(|| a.cmp(b))
}

/// Pick which of the candidates in one directory to stage, given that every one
/// of them already matched the required SONAME.
///
/// Names that are not releases are held back rather than ranked inline.
/// `cupti_version_key` maps them to `None` and `None` sorts *first*, so ranking
/// them together with real releases would make `Lowest` prefer a hand-made
/// `libcupti.so.12` over the oldest actual release — the exact outcome the
/// ranking exists to prevent. They are used only when nothing conforming is
/// present. An `Exact` pin is honoured against every candidate, conforming or
/// not, because naming a file explicitly is an operator decision.
fn pick_cupti_candidate<'a>(
    candidates: &'a [String],
    preference: &CuptiPreference,
) -> Option<&'a str> {
    let names: Vec<&'a str> = candidates.iter().map(String::as_str).collect();
    let (releases, others): (Vec<&str>, Vec<&str>) = names
        .iter()
        .copied()
        .partition(|name| cupti_version_key(name).is_some());
    let pool: &[&str] = if releases.is_empty() { &others } else { &releases };

    if let CuptiPreference::Exact(wanted) = preference {
        if let Some(hit) = names.iter().copied().find(|name| *name == wanted.as_str()) {
            return Some(hit);
        }
    }
    match preference {
        CuptiPreference::Lowest => pool
            .iter()
            .copied()
            .min_by(|&a, &b| compare_cupti_candidates(a, b)),
        CuptiPreference::Highest | CuptiPreference::Exact(_) => pool
            .iter()
            .copied()
            .max_by(|&a, &b| compare_cupti_candidates(a, b)),
    }
}

/// `DT_SONAME` of a file, memoised.
///
/// `utils::read_soname` reads the whole file to reach the dynamic section and a
/// vendored libcupti is 4-8 MB, so ranking every candidate means reading the
/// whole directory (~67 MB) once per injection. The vendored tree cannot change
/// under a running agent, so one parse per file per process is enough.
fn soname_of(path: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();

    // Canonicalise the key: in the shipped image the same directory is reached
    // both as `./cupti` and as `/opt/aiprof/cupti`, and keying on the spelling
    // would parse the same 4-8 MB binary twice.
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hit) = map.get(&key) {
            return Some(hit.clone());
        }
    }

    // Parsed outside the lock: read_soname reads the whole file, and holding a
    // process-wide mutex across an 8 MB read would serialise every caller.
    let soname = match utils::read_soname(key.to_string_lossy().as_ref()) {
        Ok(soname) => soname,
        Err(e) => {
            // Deliberately not cached. A transient failure - EMFILE under the
            // unix socket handler's per-connection churn, EIO on overlayfs -
            // would otherwise disable staging for this file for the whole
            // process lifetime and surface only as "no vendored libcupti found".
            log::warn!("cannot read the soname of {}: {}", key.display(), e);
            return None;
        }
    };
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, soname.clone());
    Some(soname)
}

/// Unique temporary name a vendored libcupti is staged under before being
/// `rename(2)`d into place: `.cupti-staging-<agent pid>-<nanos hex>-<target
/// pid>`.
///
/// It deliberately does **not** start with `libcupti.so.`, so a leftover from a
/// crash between the copy and the rename can never be picked up as a candidate
/// on a later run; the leading dot also keeps it out of a casual `ls`. Both
/// error paths in `stage_cupti_runtime` unlink it.
///
/// The one residue that cannot be covered is a `SIGKILL` landing between the
/// copy and the rename, which leaves one hidden 4-8 MB file in the target's
/// `/tmp` until that `/tmp` is reclaimed. `Drop` is no use there — it does not
/// run after `SIGKILL` — and scanning the target's `/tmp` for leftovers instead
/// would have to match on a pid that is only unique within a pid namespace, so
/// two agents sharing a target could unlink each other's in-flight staging file.
/// One stranded file is the cheaper failure.
fn cupti_staging_name(target_pid: i32, unique: u128) -> String {
    format!(
        ".cupti-staging-{}-{:x}-{}",
        process::id(),
        unique,
        target_pid
    )
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
    // form libcupti.so.<major>, then copy the highest-release vendored
    // candidate whose soname matches exactly.
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
        // Canonicalised before being de-duplicated, and that order matters. In
        // the shipped image config.yaml names `/libcuprof.so` but the first
        // candidate tried is `./libcuprof.so`, which resolves because WORKDIR is
        // /opt/aiprof; cuprof_dir is therefore `.` while current_exe() gives
        // `/opt/aiprof`. Compared as written those four entries are all
        // distinct, so the two real directories would each be scanned twice -
        // every candidate parsed twice on the no-match path, and the same path
        // listed twice in the error below.
        let mut wanted = vec![cuprof_dir.join("cupti"), cuprof_dir.clone()];
        if let Ok(exe_path) = env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                wanted.push(parent.join("cupti"));
                wanted.push(parent.to_path_buf());
            }
        }
        let mut search_dirs: Vec<PathBuf> = Vec::with_capacity(wanted.len());
        for dir in wanted {
            let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
            if !search_dirs.contains(&dir) {
                search_dirs.push(dir);
            }
        }

        let preference = cupti_preference();

        let mut chosen: Option<std::path::PathBuf> = None;
        for dir in &search_dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            // Several vendored files can carry the same SONAME - this tree
            // ships four releases whose SONAME is `libcupti.so.12` - and
            // `read_dir` yields them in directory order, which is neither
            // sorted nor stable across filesystems. Taking the first hit
            // therefore let the same client image stage CUPTI 12.1 on one host
            // and 12.8 on another, which changes the minimum driver the target
            // needs and the set of GPUs CUPTI recognises. Rank every match
            // instead, and say so in the log when the choice was not forced.
            let mut candidates: Vec<String> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("libcupti.so.") {
                    continue;
                }
                if soname_of(&entry.path()).as_deref() == Some(needed.as_str()) {
                    candidates.push(name);
                }
            }

            let best = pick_cupti_candidate(&candidates, &preference);
            if let (CuptiPreference::Exact(wanted), Some(picked)) = (&preference, best) {
                if picked != wanted.as_str() {
                    log::warn!(
                        "AIPROF_CUPTI_PREFER='{}' is not among the vendored files in {} whose \
                         soname is '{}' ({}); staging {} instead",
                        wanted,
                        dir.display(),
                        needed,
                        candidates.join(", "),
                        picked
                    );
                }
            }
            if let Some(best) = best {
                if candidates.len() > 1 {
                    log::info!(
                        "{} vendored libcupti files in {} share soname '{}', staging {} \
                         (preference {:?}, candidates: {})",
                        candidates.len(),
                        dir.display(),
                        needed,
                        best,
                        preference,
                        candidates.join(", ")
                    );
                }
                chosen = Some(dir.join(best));
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

        // Stage through a unique temporary name and rename(2) it into place.
        // libcuprof.so is linked `-z nodelete`, so a libcupti staged for an
        // earlier collection window is still mapped in the target: copying
        // straight onto the final name truncates and rewrites that live inode,
        // and the target then dies with SIGBUS on its next page-in. rename()
        // swaps the directory entry instead - the mapped inode survives intact
        // and the staged file is a fresh one. It also stops two collections
        // against targets that share a /tmp from racing on the same name. The
        // temporary name deliberately does not start with `libcupti.so.`, so a
        // leftover can never be mistaken for a staging candidate, and both error
        // paths below unlink it.
        let dst = format!("/proc/{}/root/tmp/{}", target_pid, needed);
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let staging = format!(
            "/proc/{}/root/tmp/{}",
            target_pid,
            cupti_staging_name(target_pid, unique)
        );

        if let Err(e) = utils::copy_file(chosen.to_string_lossy().as_ref(), &staging, false) {
            let _ = std::fs::remove_file(&staging);
            return Err(e).context(format!("failed to stage {} for target", needed));
        }
        if let Err(e) = std::fs::rename(&staging, &dst) {
            let _ = std::fs::remove_file(&staging);
            return Err(anyhow::Error::new(e).context(format!(
                "failed to move staged {} into place at {}",
                needed, dst
            )));
        }
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

        // Clean up socket and config files, but do NOT touch
        // libcuprof.so / libcupti.so. libcuprof.so is linked `-z nodelete`, so
        // it and the libcupti it pulled in stay mapped in the target for the
        // target's whole lifetime, and rewriting one of those files in place
        // makes the next kernel page-in raise SIGBUS and kill the customer
        // process - which is why stage_cupti_runtime() renames a fresh copy
        // into place rather than overwriting the name. Unlinking the name
        // alone would be survivable but reclaims nothing while the inode is
        // still mapped, and a staged libcupti is 4-8 MB rather than the <1MB
        // this cleanup used to assume. Once the target exits,
        // /proc/<pid>/root is gone and the files are reclaimed with it.
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

#[cfg(test)]
mod tests {
    use super::{
        compare_cupti_candidates, cupti_staging_name, cupti_version_key, parse_cupti_preference,
        pick_cupti_candidate, CuptiPreference,
    };

    // Four of the ten vendored binaries carry SONAME `libcupti.so.12`
    // (2023.2.1, 2024.1.1, 2024.3.2, 2025.2.1), so which one gets staged used to
    // be decided by `read_dir` order. These tests pin the ranking down without a
    // GPU, an ELF, or a target process.

    const SONAME_12: [&str; 4] = [
        "libcupti.so.2024.3.2",
        "libcupti.so.2023.2.1",
        "libcupti.so.2025.2.1",
        "libcupti.so.2024.1.1",
    ];

    fn owned(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn version_key_orders_releases_numerically_not_lexically() {
        // A string compare puts "9.0.176" after "2025.2.1" (because '9' > '2')
        // and "10.2.75" before "9.0.176". Both are wrong for version ordering.
        assert!("libcupti.so.9.0.176" > "libcupti.so.2025.2.1");
        assert!(
            cupti_version_key("libcupti.so.10.2.75") > cupti_version_key("libcupti.so.9.0.176")
        );
        assert!(
            cupti_version_key("libcupti.so.2025.2.1") > cupti_version_key("libcupti.so.9.0.176")
        );
        assert!(
            cupti_version_key("libcupti.so.2024.3.2") > cupti_version_key("libcupti.so.2024.1.1")
        );
    }

    #[test]
    fn a_name_that_is_not_a_release_ranks_below_every_real_one() {
        // `libcupti.so.12` is a SONAME, not a release. Compared as a version it
        // would be [12] and lose to [2023, 2, 1], silently staging an older
        // CUPTI, so it is not treated as a version at all.
        assert_eq!(cupti_version_key("libcupti.so.12"), None);
        assert_eq!(cupti_version_key("libcupti.so.bogus"), None);
        assert!(cupti_version_key("libcupti.so.9.0.176") > cupti_version_key("libcupti.so.12"));

        // `None` sorts *first* ascending, so a non-release name has to be held
        // back rather than ranked inline: otherwise Highest skips it correctly
        // but Lowest picks it, which is the very outcome the ranking prevents.
        let mixed = owned(&["libcupti.so.12", "libcupti.so.2023.2.1", "libcupti.so.2025.2.1"]);
        assert_eq!(
            pick_cupti_candidate(&mixed, &CuptiPreference::Highest),
            Some("libcupti.so.2025.2.1")
        );
        assert_eq!(
            pick_cupti_candidate(&mixed, &CuptiPreference::Lowest),
            Some("libcupti.so.2023.2.1")
        );
        // With nothing conforming present, a non-release name is still better
        // than failing the injection.
        assert_eq!(
            pick_cupti_candidate(&owned(&["libcupti.so.12"]), &CuptiPreference::Lowest),
            Some("libcupti.so.12")
        );
    }

    #[test]
    fn an_exact_pin_is_honoured_even_for_a_name_that_is_not_a_release() {
        // Naming a file explicitly is an operator decision, so it is looked up
        // against every candidate rather than only the ranked ones.
        assert_eq!(
            pick_cupti_candidate(
                &owned(&["libcupti.so.12", "libcupti.so.2023.2.1"]),
                &CuptiPreference::Exact("libcupti.so.12".to_string())
            ),
            Some("libcupti.so.12")
        );
    }

    #[test]
    fn the_highest_release_wins_whatever_order_the_directory_yielded() {
        let mut rotated = owned(&SONAME_12);
        for _ in 0..SONAME_12.len() {
            assert_eq!(
                pick_cupti_candidate(&rotated, &CuptiPreference::Highest),
                Some("libcupti.so.2025.2.1")
            );
            rotated.rotate_left(1);
        }
        let mut sorted = owned(&SONAME_12);
        sorted.sort();
        assert_eq!(
            pick_cupti_candidate(&sorted, &CuptiPreference::Highest),
            Some("libcupti.so.2025.2.1")
        );
        // The order is total, so min and max are the two ends of one ordering.
        assert!(compare_cupti_candidates("libcupti.so.2023.2.1", "libcupti.so.2025.2.1").is_lt());
    }

    #[test]
    fn the_lowest_preference_picks_the_oldest_release() {
        // The escape hatch for a fleet whose drivers predate the newest patch
        // level in a SONAME family: CUPTI's minimum driver rises with it.
        assert_eq!(
            pick_cupti_candidate(&owned(&SONAME_12), &CuptiPreference::Lowest),
            Some("libcupti.so.2023.2.1")
        );
    }

    #[test]
    fn an_exact_preference_pins_a_named_release() {
        assert_eq!(
            pick_cupti_candidate(
                &owned(&SONAME_12),
                &CuptiPreference::Exact("libcupti.so.2024.1.1".to_string())
            ),
            Some("libcupti.so.2024.1.1")
        );
    }

    #[test]
    fn an_exact_preference_that_is_not_present_falls_back_to_the_default() {
        // Naming a release that is not vendored, or one whose SONAME does not
        // match, must not fail the injection: fall back and warn instead.
        assert_eq!(
            pick_cupti_candidate(
                &owned(&SONAME_12),
                &CuptiPreference::Exact("libcupti.so.2025.3.1".to_string())
            ),
            Some("libcupti.so.2025.2.1")
        );
    }

    #[test]
    fn a_lone_candidate_wins_and_an_empty_set_yields_none() {
        assert_eq!(
            pick_cupti_candidate(&owned(&["libcupti.so.2025.3.1"]), &CuptiPreference::Highest),
            Some("libcupti.so.2025.3.1")
        );
        assert_eq!(pick_cupti_candidate(&[], &CuptiPreference::Highest), None);
        assert_eq!(pick_cupti_candidate(&[], &CuptiPreference::Lowest), None);
    }

    #[test]
    fn preference_parsing_defaults_to_highest_and_rejects_junk() {
        assert_eq!(parse_cupti_preference(None), CuptiPreference::Highest);
        assert_eq!(parse_cupti_preference(Some("")), CuptiPreference::Highest);
        assert_eq!(parse_cupti_preference(Some("  ")), CuptiPreference::Highest);
        assert_eq!(
            parse_cupti_preference(Some("highest")),
            CuptiPreference::Highest
        );
        assert_eq!(
            parse_cupti_preference(Some(" lowest ")),
            CuptiPreference::Lowest
        );
        assert_eq!(
            parse_cupti_preference(Some("libcupti.so.2023.2.1")),
            CuptiPreference::Exact("libcupti.so.2023.2.1".to_string())
        );
        // A typo is not a policy: warn and keep the default.
        assert_eq!(
            parse_cupti_preference(Some("newest")),
            CuptiPreference::Highest
        );
        assert_eq!(
            parse_cupti_preference(Some("/etc/passwd")),
            CuptiPreference::Highest
        );
    }

    #[test]
    fn the_staging_name_can_never_be_mistaken_for_a_candidate() {
        // The whole point of staging through a temporary name is that a leftover
        // cannot be picked up by a later run, which only holds if the name is
        // outside the `libcupti.so.` filter the scan uses.
        let name = cupti_staging_name(4242, 0xdeadbeef);
        assert!(!name.starts_with("libcupti.so."), "{}", name);
        assert!(name.starts_with(".cupti-staging-"), "{}", name);
        assert!(
            name.contains(&format!("{}", std::process::id())),
            "{}",
            name
        );
        assert!(name.contains("4242"), "{}", name);
        // Two calls with different uniques must not collide.
        assert_ne!(name, cupti_staging_name(4242, 0xdeadbef0));
    }
}
