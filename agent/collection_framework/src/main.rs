// src/main.rs

mod collector;
mod command;
mod config;
mod r#const;
mod detector;
mod error;
mod plugins;
mod tools;
mod unix_handler;
#[allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
mod injector {
    include!(concat!(env!("OUT_DIR"), "/injector.rs"));
}
mod meta;

use crate::tools::injector_handle;
use crate::tools::memory_monitor::MemoryMonitor;
use crate::{
    collector::scheduler::CollectorScheduler,
    command::{Args, Commands},
    error::ErrorCode,
    tools::utils,
};
use clap::Parser;
use tracing::{self as log, debug};
use tracing_subscriber::{
    filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
    fmt,
};
use std::sync::Arc;
use std::fs::{File, OpenOptions};
use std::io::Write;

// Enumerate direct-child PIDs of the current process via procfs and SIGKILL
// them. Called right before std::process::exit(0) in the Profile handler:
// the pyki plugin's injector task uses sync std::process::Command +
// blocking child.wait() inside tokio::spawn, so scheduler shutdown can't
// cancel it. Reaping here prevents leaking the pyki-inject grandchild
// (which would otherwise reparent to init and keep pinning target-process
// state that libprofiler.a set up).
fn reap_direct_children() {
    let mypid = std::process::id();
    // /proc/self/task/<tid>/children contains SPACE-separated pids of
    // children reaped by each thread; union across all threads.
    let task_dir = format!("/proc/{}/task", mypid);
    let mut victims: Vec<i32> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&task_dir) {
        for entry in entries.flatten() {
            let children_path = entry.path().join("children");
            if let Ok(s) = std::fs::read_to_string(&children_path) {
                for tok in s.split_whitespace() {
                    if let Ok(pid) = tok.parse::<i32>() {
                        if pid > 0 && !victims.contains(&pid) {
                            victims.push(pid);
                        }
                    }
                }
            }
        }
    }
    for pid in &victims {
        log::info!("Reaping surviving child PID {} before exit", pid);
        unsafe { libc::kill(*pid, libc::SIGKILL); }
    }
    // Best-effort brief drain so we don't leave zombies.
    if !victims.is_empty() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut remaining = victims.len();
        while remaining > 0 && std::time::Instant::now() < deadline {
            let mut status = 0i32;
            let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if r > 0 {
                remaining = remaining.saturating_sub(1);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
}

fn set_logger(verbose: u8, log_dir: Option<&str>) {
    let log_level = match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };

    // Create layers for stdout
    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout);

    // Initialize subscriber based on whether log_dir is provided
    if let Some(log_dir) = log_dir {
        // Create log directory if it doesn't exist
        let _ = std::fs::create_dir_all(log_dir);

        // Create log file with date
        let now = chrono::Local::now();
        let log_file_name = format!("{}/CollectionFramework-{}.log", log_dir, now.format("%Y-%m-%d"));
        
        let log_file = Arc::new(std::sync::Mutex::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file_name)
                .expect("Failed to open log file")
        ));

        // Create a file layer with custom writer
        let file_layer = fmt::layer()
            .with_writer(move || -> Box<dyn Write + Send> {
                struct FileWriter(Arc<std::sync::Mutex<File>>);
                impl Write for FileWriter {
                    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                        self.0.lock().unwrap().write(buf)
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        self.0.lock().unwrap().flush()
                    }
                }
                Box::new(FileWriter(Arc::clone(&log_file)))
            })
            .with_ansi(false); // Disable ANSI colors in file

        // Initialize subscriber with both stdout and file layers
        let _ = tracing_subscriber::registry()
            .with(stdout_layer)
            .with(file_layer)
            .with(
                EnvFilter::builder()
                    .with_default_directive(log_level.into())
                    .from_env_lossy(),
            )
            .init();
    } else {
        // Initialize subscriber with only stdout layer
        let _ = tracing_subscriber::registry()
            .with(stdout_layer)
            .with(
                EnvFilter::builder()
                    .with_default_directive(log_level.into())
                    .from_env_lossy(),
            )
            .init();
    }
}

// worker_threads is pinned to a floor of 4 so the runtime never starves in
// docker containers with CPU quota=1. Historically we relied on the default
// (= num_cpus), which meant a single worker had to co-drive:
//   1. scheduler event loop (blocks on std::sync::mpsc::recv_timeout for up
//      to duration + 60s while waiting for WritingFinish),
//   2. auto_stop_timer's sleep().await (must wake to send StopTimedCollector),
//   3. pyki-inject / cupti-inject supervisor tasks (each blocks on
//      std::process::Command::wait() for the whole profiling window).
// With only one worker the recv_timeout would pin the sole thread and every
// other future stopped being polled — auto_stop never fired, the injector
// tasks never started, and the run timed out with an empty tar. Bumping to
// 4 workers is a defensive floor; the correct blocking calls are ALSO moved
// off worker threads via spawn_blocking (see scheduler.rs and
// pyki_plugin_wrapper.rs).
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> ErrorCode {
    // async fn main() {
    // Check whether we have root privileges
    if !tools::utils::is_root() {
        log::error!("need root permission to run this program");
        return ErrorCode::PermissionDenied(None);
    }

    let args = Args::parse();
    set_logger(args.verbose, args.log_dir.as_deref());

    match args.command {
        Commands::Profile(mut args) => {
            // Start memory monitoring (debug mode only)
            let mut memory_monitor = {
                let mut monitor = MemoryMonitor::new();
                monitor.start(100, Some(2 * 1024 * 1024)); // Monitor every 100ms, 2GB limit
                monitor
            };

            if let Err(e) = args.validate() {
                log::error!("Parameter verification failed: {}", e);
                {
                    memory_monitor.stop().await;
                    memory_monitor.print_peak_memory();
                }
                return e;
            }
            debug!("Args: {:#?}", args);

            let config = match config::Config::from_path(args.config_path.clone().unwrap()) {
                Ok(config) => config,
                Err(e) => {
                    log::error!("Collect Failed: {}", e);
                    {
                        memory_monitor.stop().await;
                        memory_monitor.print_peak_memory();
                    }
                    return ErrorCode::ConfigNotFound(Some(e.to_string()));
                }
            };

            let mut scheduler = match unsafe { CollectorScheduler::new(config, args.clone()) } {
                Ok(scheduler) => scheduler,
                Err(e) => {
                    log::error!("Collect Failed: {}", e);
                    {
                        memory_monitor.stop().await;
                        memory_monitor.print_peak_memory();
                    }
                    return ErrorCode::CreateSchedulerError(Some(e.to_string()));
                }
            };

            match scheduler.init().await {
                Ok(_) => log::info!("All collectors initialized successfully."),
                Err(e) => {
                    log::error!("Collect Failed: {}", e);
                    {
                        memory_monitor.stop().await;
                        memory_monitor.print_peak_memory();
                    }
                    return ErrorCode::InitCollectorError(Some(e.to_string()));
                }
            }

            match scheduler.start_event_loop(&args).await {
                Ok(_) => log::info!("Scheduler event loop exited."),
                Err(e) => {
                    log::error!("Collect Failed: {}", e);
                    {
                        memory_monitor.stop().await;
                        memory_monitor.print_peak_memory();
                    }
                    return ErrorCode::SchedulerError(Some(e.to_string()));
                }
            }

            {
                // Stop memory monitoring and print results
                memory_monitor.stop().await;
                memory_monitor.print_peak_memory();
            }

            // The pyki plugin spawns its `pyki-inject` grandchild from a
            // detached tokio task using sync std::process::Command +
            // blocking child.wait(); that task can't be cancelled by the
            // scheduler's manager.drop(), so returning from main here
            // would let the tokio runtime stay alive holding the FD.
            // Reap our direct children (pyki-inject only) and exit the
            // process explicitly — the client will observe CF terminate
            // and clear TaskRunner regardless of any stray blocking task.
            reap_direct_children();
            log::info!("Program exited successfully.");
            std::process::exit(0);
        }

        Commands::PykiInject(args) => {
            let python_code = format!(
                "from pyki.{}.profiling.torch_profile import execute\n\
            import socket\n\
            import os\n\
            from datetime import datetime\n\
            def send_message(message):\n\
                \tsock_file = \"{}\"\n\
                \tclient = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\n\
                \ttry:\n\
                    \t\tclient.connect(sock_file)\n\
                    \t\tclient.sendall(message.encode('utf-8'))\n\
                \texcept Exception as e:\n\
                    \t\tprint(f\"Send message error: {{e}}\")\n\
                \tfinally:\n\
                    \t\tclient.close()\n\
            \n\
            def on_start():\n\
                \tnow = datetime.now()\n\
                \tprint(\"start time is \")\n\
                \tprint(now.strftime(\"%Y-%m-%d %H:%M:%S\"))\n\
                \tsend_message(\"{}\")\n\
            \n\
            def on_stop():\n\
                \tnow = datetime.now()\n\
                \tprint(\"stop time is \")\n\
                \tprint(now.strftime(\"%Y-%m-%d %H:%M:%S\"))\n\
                \tsend_message(\"{}\")\n\
            \n\
            result = execute({})\n\
            print(result)\n",
                r#const::PYKI_VERSION,
                format!("{}{}", r#const::CF_UNIXSOCK, args.target_pid),
                r#const::START_PYKI_COLLECTOR,
                r#const::STOP_PYKI_COLLECTOR,
                args.pyki_args,
            );

            let output_path = args.output.clone().unwrap();
            let result = std::panic::catch_unwind(|| {
                injector_handle::inject_pyki_loader_static(python_code, args.target_pid)
            });

            match result {
                Ok(Ok(())) => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

                    if output_path != "default" {
                        // Scope both globs to this target's own pid AND anchor
                        // them at the delimiter that follows it:
                        //
                        //   * The sweep is destructive (a move), so the previous
                        //     unscoped `AIProf_*` pattern republished every
                        //     leftover trace in the target's /tmp as this run's
                        //     result and unlinked the source.
                        //   * `AIProf_<pid>*` still over-matched a longer pid
                        //     sharing the same digit prefix (pid 123 swept
                        //     AIProf_1234_cupti.json). pyki separates with '-':
                        //     its prefix is `AIProf_<pid>` and gen_result_path
                        //     builds `<prefix>-<ns-pid>-<id>-<suffix>`.
                        //
                        // Unlike the cupti branch, finding nothing here IS worth
                        // surfacing: inject_pyki_loader_static blocks for the
                        // whole collection window (loader.c runs
                        // PyRun_SimpleString synchronously and injector_handle
                        // waits on THREAD_END), so pyki's files already exist
                        // when this 3s sleep ends, and this sweep is the only
                        // thing that moves them into the directory the parent
                        // later globs - there is no custom_shutdown copy and no
                        // mtime guard on this path. An empty result means this
                        // pid's whole pyki report is missing, so warn; the
                        // default --verbose 1 is INFO and would hide debug.
                        let json_pattern =
                            utils::pid_artifact_pattern(args.target_pid, '-', "*.json");
                        match utils::move_matching(&json_pattern, &output_path) {
                            Ok(0) => log::warn!(
                                "pyki produced no json output for pid {} ({} matched nothing); \
                                 this pid's report will be empty",
                                args.target_pid,
                                json_pattern
                            ),
                            Ok(n) => log::info!(
                                "Collected {} pyki json file(s) for pid {}",
                                n,
                                args.target_pid
                            ),
                            Err(e) => log::warn!(
                                "Failed to collect pyki json output {}: {}",
                                json_pattern,
                                e
                            ),
                        }

                        let pickle_pattern = utils::pid_artifact_pattern(
                            args.target_pid,
                            '-',
                            "*cuda-memory.pickle",
                        );
                        match utils::move_matching(&pickle_pattern, &output_path) {
                            // The pickle only exists for a snapshot run, so zero
                            // is expected in most configurations.
                            Ok(0) => log::debug!(
                                "No pyki memory pickle for pid {} ({})",
                                args.target_pid,
                                pickle_pattern
                            ),
                            Ok(n) => log::info!(
                                "Collected {} pyki pickle file(s) for pid {}",
                                n,
                                args.target_pid
                            ),
                            Err(e) => log::warn!(
                                "Failed to collect pyki memory pickle {}: {}",
                                pickle_pattern,
                                e
                            ),
                        }
                    }

                    return ErrorCode::Success;
                }
                Ok(Err(e)) => {
                    log::error!(
                        "Failed to inject loader into process {}: {}",
                        args.target_pid,
                        e
                    );
                    return ErrorCode::InjectFailed(None);
                }
                Err(_) => {
                    log::error!(
                        "Thread panicked while injecting loader into process {}",
                        args.target_pid
                    );
                    return ErrorCode::InjectFailed(None);
                }
            };
        }

        Commands::CuptiInject(args) => {
            let output_path = args.output.clone().unwrap_or_else(|| ".".to_string());
            let result = std::panic::catch_unwind(|| {
                injector_handle::inject_cupti_loader_static(args.target_pid)
            });

            match result {
                Ok(Ok(())) => {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

                    if output_path != "default" {
                        // Scope the glob to this target's own pid and anchor it
                        // at the '_' delimiter. The bare `AIProf_*.json` pattern
                        // swept *every* leftover trace in the target's /tmp into
                        // this run's output directory and then deleted the source
                        // (copy_file(delete=true) is a move), so an unrelated
                        // earlier run's trace - possibly for a long-dead pid -
                        // was republished as this task's data, bypassing the
                        // mtime stale-guard in
                        // cupti_plugin_wrapper.rs::custom_shutdown. The narrower
                        // `AIProf_<pid>*` was still too loose: it also matches a
                        // longer pid sharing the same digit prefix. cuprof writes
                        // exactly `AIProf_<pid>_cupti.json`, so require the '_'.
                        let pattern = utils::pid_artifact_pattern(args.target_pid, '_', "*.json");
                        // Zero matches is the normal outcome here: cuprof writes
                        // only when the window closes, usually after this 3s
                        // sweep, and the parent then copies the trace itself
                        // under an mtime guard. Keep 0 at debug, but a real I/O
                        // failure must stay visible at the default log level.
                        match utils::move_matching(&pattern, &output_path) {
                            Ok(0) => {
                                log::debug!("No cuprof output to collect yet ({})", pattern)
                            }
                            Ok(n) => log::info!("Collected {} cuprof trace file(s)", n),
                            Err(e) => {
                                log::warn!("Failed to collect cuprof output {}: {}", pattern, e)
                            }
                        }
                    }

                    return ErrorCode::Success;
                }
                Ok(Err(e)) => {
                    log::error!(
                        "Failed to inject loader into process {}: {}",
                        args.target_pid,
                        e
                    );
                    return ErrorCode::InjectFailed(None);
                }
                Err(_) => {
                    log::error!(
                        "Thread panicked while injecting loader into process {}",
                        args.target_pid
                    );
                    return ErrorCode::InjectFailed(None);
                }
            }
        }
    }
}
