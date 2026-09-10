// src/plugins/unix_handler.rs
use crate::collector::collector_name::CollectorName;
use crate::collector::event_handler::{EventHandler, SchedulerEvent};
use crate::r#const;
use std::fs;
use std::path::Path;
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tracing as log;
pub struct UnixSocketHandler;

/// Map a lifecycle message received on a target's unix socket to the scheduler
/// event it stands for.
///
/// These strings are a wire protocol with the injected collectors, matched by
/// exact equality:
///
///   * cuprof (vendored at `src/plugins/cuprof`) emits `CUPTIProfilingStart` /
///     `Stop` / `WriterOver` / `Failed` from `src/cupti_sink.cc`. One connection
///     per message, no framing - see `src/plugins/cuprof/docs/embedding.md`.
///   * the pyki loader emits `STARTPYKICOLLECTOR` / `STOPPYKICOLLECTOR` from the
///     Python snippet built in `main.rs`.
///
/// The `*_drift_guard` tests at the bottom of this file fail if a re-sync of the
/// vendored cuprof tree renames any of them, which is the check
/// `src/plugins/cuprof/VENDOR.md` otherwise asks a human to perform by hand.
pub fn scheduler_event_for(received: &str, pid: i32) -> Option<SchedulerEvent> {
    match received {
        r#const::START_PYKI_COLLECTOR => Some(SchedulerEvent::StartPendingCollector(
            CollectorName::Pyki,
            pid,
        )),
        r#const::STOP_PYKI_COLLECTOR => {
            Some(SchedulerEvent::StopCollector(CollectorName::Pyki, pid))
        }
        r#const::START_CUPTI_COLLECTOR => Some(SchedulerEvent::StartPendingCollector(
            CollectorName::CUPTI,
            pid,
        )),
        r#const::STOP_CUPTI_COLLECTOR => {
            Some(SchedulerEvent::StopCollector(CollectorName::CUPTI, pid))
        }
        r#const::FINISH_CUPTI_COLLECTOR => {
            Some(SchedulerEvent::WritingFinish(CollectorName::CUPTI, pid))
        }
        // cuprof sends this *instead of* WriterOver when it could not write the
        // trace file. embedding.md asks consumers to treat it as terminal for
        // the window and not to wait for a file that will never appear.
        // CollectFailed drives writer::handle_collect_failed, which sets
        // CollectorState::Failed (8); all_collectors_finished() only requires
        // >= WrittingOver (7), so the run finalizes immediately instead of
        // idling until the duration + 60s watchdog fires.
        r#const::FAILED_CUPTI_COLLECTOR => {
            Some(SchedulerEvent::CollectFailed(CollectorName::CUPTI, pid))
        }
        _ => None,
    }
}

impl UnixSocketHandler {
    pub fn start_listen(
        pid: i32,
        shutdown_sender: broadcast::Sender<()>,
    ) -> tokio::task::JoinHandle<()> {
        let socket_path = format!("/proc/{}/root{}{}", pid, r#const::CF_UNIXSOCK, pid);

        tokio::spawn(async move {
            log::debug!("Starting Listen Unixsock for pid: {}", pid);

            let listener = {
                if Path::new(&socket_path).exists() {
                    if let Err(e) = fs::remove_file(&socket_path) {
                        log::error!("Failed to remove existing socket file: {}", e);
                    }
                }

                match UnixListener::bind(&socket_path) {
                    Ok(listener) => listener,
                    Err(e) => {
                        log::error!("Failed to bind Unix socket: {}", e);
                        return;
                    }
                }
            };

            // Create a receiver to listen for the shutdown signal
            let mut shutdown_receiver = shutdown_sender.subscribe();

            // Loop forever, listening for new connections
            loop {
                tokio::select! {
                    // Accept a new connection and handle messages
                    result = listener.accept() => {
                        match result {
                            Ok((socket, _addr)) => {
                                log::debug!("Client connected to Unix socket for pid: {}", pid);

                                // Subscribe to the shutdown signal for each connection
                                let mut conn_shutdown_receiver = shutdown_sender.subscribe();

                                // Spawn an async task per connection to handle multiple messages
                                tokio::spawn(async move {
                                    loop {
                                        tokio::select! {
                                            result = socket.readable() => {
                                                match result {
                                                    Ok(()) => {
                                                        let mut buffer = vec![0; 1024];
                                                        match socket.try_read(&mut buffer) {
                                                            Ok(0) => {
                                                                // Connection closed
                                                                log::debug!("Client disconnected from Unix socket for pid: {}", pid);
                                                                break;
                                                            }
                                                            Ok(n) => {
                                                                // Convert the read data to a string and strip trailing NUL bytes
                                                                let received_data = String::from_utf8_lossy(&buffer[..n]);
                                                                let received = received_data.trim_end_matches('\0');
                                                                log::debug!("Received UnixSocket message: {} for pid: {}", received, pid);

                                                                // Dispatch the matching event based on the received message
                                                                if received == r#const::FAILED_CUPTI_COLLECTOR {
                                                                    // Raw messages are only logged at debug, so surface
                                                                    // the one that means this window produced no data.
                                                                    log::warn!(
                                                                        "cuprof could not write its trace for pid {}; this collection window produced no output",
                                                                        pid
                                                                    );
                                                                }
                                                                let event = scheduler_event_for(received, pid);

                                                                // If there is a matching event, send it
                                                                if let Some(evt) = event {
                                                                    if let Err(e) = EventHandler::global_sender().send(evt) {
                                                                        log::error!("Failed to send scheduler event: {}", e);
                                                                    }
                                                                }
                                                            }
                                                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                                                continue;
                                                            }
                                                            Err(e) => {
                                                                log::error!("Failed to read from Unix socket for pid {}: {}", pid, e);
                                                                break;
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!("Failed to make Unix socket readable for pid {}: {}", pid, e);
                                                        break;
                                                    }
                                                }
                                            }
                                            // Listen for a per-connection shutdown signal
                                            _ = conn_shutdown_receiver.recv() => {
                                                log::info!("Received shutdown signal, stopping connection handler for pid: {}", pid);
                                                break;
                                            }
                                        }
                                    }
                                });
                            }
                            Err(e) => log::error!("Failed to accept Unix socket connection: {}", e),
                        }
                    }
                    // Listen for the shutdown signal
                    _ = shutdown_receiver.recv() => {
                        log::error!("Received shutdown signal, stopping Unix socket listener for pid: {}", pid);
                        break;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuprof_lifecycle_messages_map_to_scheduler_events() {
        assert!(matches!(
            scheduler_event_for(r#const::START_CUPTI_COLLECTOR, 42),
            Some(SchedulerEvent::StartPendingCollector(
                CollectorName::CUPTI,
                42
            ))
        ));
        assert!(matches!(
            scheduler_event_for(r#const::STOP_CUPTI_COLLECTOR, 42),
            Some(SchedulerEvent::StopCollector(CollectorName::CUPTI, 42))
        ));
        assert!(matches!(
            scheduler_event_for(r#const::FINISH_CUPTI_COLLECTOR, 42),
            Some(SchedulerEvent::WritingFinish(CollectorName::CUPTI, 42))
        ));
    }

    #[test]
    fn cuprof_failed_write_is_terminal_and_not_silently_dropped() {
        // Before this mapping existed the message fell through to `None`, so no
        // event was emitted and the scheduler waited out its whole
        // duration + 60s watchdog for a trace that cuprof had already said it
        // could not produce.
        assert!(matches!(
            scheduler_event_for(r#const::FAILED_CUPTI_COLLECTOR, 42),
            Some(SchedulerEvent::CollectFailed(CollectorName::CUPTI, 42))
        ));
    }

    #[test]
    fn pyki_lifecycle_messages_map_to_scheduler_events() {
        assert!(matches!(
            scheduler_event_for(r#const::START_PYKI_COLLECTOR, 42),
            Some(SchedulerEvent::StartPendingCollector(
                CollectorName::Pyki,
                42
            ))
        ));
        assert!(matches!(
            scheduler_event_for(r#const::STOP_PYKI_COLLECTOR, 42),
            Some(SchedulerEvent::StopCollector(CollectorName::Pyki, 42))
        ));
    }

    #[test]
    fn the_socket_pid_is_carried_into_the_event() {
        // The socket is per-target; attributing an event to the wrong pid would
        // credit one process's kernels to another.
        assert!(matches!(
            scheduler_event_for(r#const::FINISH_CUPTI_COLLECTOR, 4242),
            Some(SchedulerEvent::WritingFinish(CollectorName::CUPTI, 4242))
        ));
    }

    #[test]
    fn unknown_or_empty_message_is_ignored() {
        assert!(scheduler_event_for("", 7).is_none());
        assert!(scheduler_event_for("CUPTIProfilingWhatever", 7).is_none());
        assert!(scheduler_event_for("cuptiprofilingstart", 7).is_none());
    }

    // ---- drift guards against the vendored cuprof tree ----
    // VENDOR.md asks whoever re-syncs cuprof to re-check the message names and
    // the config keys by hand. These two tests make `cargo test` fail instead.

    #[test]
    fn drift_guard_vendored_cuprof_emits_every_message_we_match_on() {
        let sink = include_str!("plugins/cuprof/src/cupti_sink.cc");
        for msg in [
            r#const::START_CUPTI_COLLECTOR,
            r#const::STOP_CUPTI_COLLECTOR,
            r#const::FINISH_CUPTI_COLLECTOR,
            r#const::FAILED_CUPTI_COLLECTOR,
        ] {
            assert!(
                sink.contains(msg),
                "vendored cuprof no longer emits {:?}: update src/const.rs and \
                 scheduler_event_for (see src/plugins/cuprof/VENDOR.md)",
                msg
            );
        }
    }

    #[test]
    fn drift_guard_vendored_cuprof_reads_every_config_key_we_write() {
        let cfg = include_str!("plugins/cuprof/src/config.cc");
        // The keys cupti_plugin_wrapper::write_cuprof_config emits into
        // /tmp/cuprof_<container-pid>.cfg, plus CUPROF_CONFIG, which
        // cuprof/VENDOR.md documents as part of the same contract.
        for key in [
            "CUPROF_OUTPUT",
            "CUPROF_DURATION",
            "CUPROF_VERBOSE",
            "CUPROF_SOCKET",
            "CUPROF_CONFIG",
        ] {
            assert!(
                cfg.contains(key),
                "vendored cuprof no longer reads {}: update \
                 cupti_plugin_wrapper::write_cuprof_config",
                key
            );
        }
    }

    #[test]
    fn drift_guard_vendored_cuprof_keeps_the_cfg_filename_convention() {
        // write_cuprof_config writes /proc/<pid>/root/tmp/cuprof_<ns-pid>.cfg
        // and depends on cuprof's LoadCfgFile looking for exactly that name.
        // A rename here is the quietest failure of the three: the injected
        // library finds no config and silently falls back to its defaults
        // (./cuprof_<pid>.json, no socket), so the run produces no trace and
        // no lifecycle message at all.
        let cfg = include_str!("plugins/cuprof/src/config.cc");
        assert!(
            cfg.contains("/tmp/cuprof_"),
            "vendored cuprof no longer reads /tmp/cuprof_<pid>.cfg: update \
             cupti_plugin_wrapper::write_cuprof_config"
        );
    }
}
