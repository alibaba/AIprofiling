use crate::collector::collector_manager::CollectorManager;
use crate::collector::collector_name::CollectorName;
use crate::collector::event_handler::{EventHandler, SchedulerEvent};
use crate::collector::indicator_name;
use crate::command::ProfileArgs;
use crate::config::Config;
use crate::error::ErrorCode;
use crate::plugins::plugin_adapter::CollectorState;
use crate::tools::writer::Writer;
use anyhow::Result;
use log::{error, info};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing as log;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulePolicy {
    Immediate = 2,       // Start immediately: GPU kernel metrics
    SignalTriggered = 1, // Wait for a triggering signal
    Timed = 0,           // Start immediately, but requires a shutdown timer
}

pub struct CollectorScheduler {
    /// Collector manager
    manager: CollectorManager,
    /// Scheduling policy per collector
    policies: HashMap<i32, HashMap<CollectorName, SchedulePolicy>>,

    args: ProfileArgs,

    config: Config,

    // Per-pid per-collector state is tracked in the writer
    writer: Writer,
}

impl CollectorScheduler {
    pub unsafe fn new(config: Config, mut args: ProfileArgs) -> Result<Self> {
        let manager = CollectorManager::new(&config, &mut args)?;
        let writer = Writer::new(manager.get_pid_map());

        let mut scheduler = Self {
            manager,
            policies: HashMap::new(),
            args,
            config,
            writer,
        };

        scheduler.setup_default_policies();

        if scheduler.policies.is_empty() {
            let error_msg = "No process meets the conditions of the collector.";
            error!("{}", error_msg);
            return Err(ErrorCode::DetectEnvError(Some(error_msg.to_string())).into());
        }

        Ok(scheduler)
    }

    /*
     * Scheduling policy (Pyki and CUPTI decoupled, changed 2026-08):
     * - CUPTI: Immediate. cuprof handles its own duration/auto-exit loop;
     *   the scheduler only needs to fire StartCollector once.
     * - Pyki: Timed. pyki-inject is a fire-and-forget tokio subtask; the
     *   scheduler needs auto_stop_timer to send StopTimedCollector when the
     *   duration elapses so that Pyki shutdown gets a chance to flush the
     *   torch-profile output.
     *
     * Rationale: the old design picked policy by indicator (GPU -> Immediate,
     * others -> SignalTriggered). Pyki carrying a GPU indicator was collapsed
     * into Immediate, so no auto_stop_timer fired shutdown, the pyki JSON was
     * never read back by CF, and the scheduler spun idle for 63s before
     * hitting the idle timeout. Deciding by collector name decouples this
     * cleanly.
     */
    fn setup_default_policies(&mut self) {
        for (pid, collectors) in self.manager.get_pid_map() {
            for (_indicator_name, collector_name) in collectors {
                let policy = match collector_name {
                    CollectorName::CUPTI => SchedulePolicy::Immediate,
                    CollectorName::Pyki => SchedulePolicy::Timed,
                    _ => SchedulePolicy::Timed,
                };

                let pid_policies = self.policies.entry(*pid).or_insert_with(HashMap::new);
                if let Some(existing_policy) = pid_policies.get(collector_name) {
                    // If a policy already exists, only overwrite when the new one has higher priority.
                    if policy as i32 > *existing_policy as i32 {
                        pid_policies.insert(*collector_name, policy);
                    }
                } else {
                    // Otherwise, insert directly.
                    pid_policies.insert(*collector_name, policy);
                }
            }
        }
        log::info!("Collector scheduler policies: {:?}", self.policies);
    }

    pub async fn init(&mut self) -> Result<()> {
        info!("Initializing collectors with scheduler");
        self.manager
            .init_collectors(&self.args, &self.config, &mut self.writer)
            .await?;
        let sender = EventHandler::global_sender();

        for (pid, policies) in &self.policies {
            let mut immediate_flag = false;

            for (collector_name, policy) in policies {
                match policy {
                    SchedulePolicy::Immediate => {
                        sender.send(SchedulerEvent::StartCollector(*collector_name, *pid))?;
                    }
                    SchedulePolicy::SignalTriggered => {
                        // Note: do nothing
                    }
                    SchedulePolicy::Timed => {
                        sender.send(SchedulerEvent::StartCollector(*collector_name, *pid))?;
                        immediate_flag = true;
                    }
                }
            }

            if immediate_flag {
                self.start_auto_stop_timer(Duration::from_secs(self.args.duration as u64), *pid);
            }
        }

        Ok(())
    }

    async fn handle_event(&mut self, event: SchedulerEvent, args: &ProfileArgs) -> Result<bool> {
        log::warn!("Handling event: {:?}", event);
        match event {
            SchedulerEvent::StartCollector(name, pid) => {
                self.writer
                    .update_or_insert_collector_state(pid, name, CollectorState::Collecting)
                    .await;
                self.manager.trigger_collect(&name, &self.args, pid).await?;
            }
            SchedulerEvent::StopTimedCollector(pid) => {
                // Stopping is not the same as having written: the collector
                // still has to flush its trace and report WritingFinish. This
                // used to stamp WrittingOver here, which made
                // is_all_pid_finish believe every timed pid was done as soon
                // as the duration elapsed — so the first pid to finish
                // writing triggered Exit and every other pid's data was
                // dropped on the floor, unmerged. Mirror StopCollector and
                // stamp UnWritting; only WritingFinish promotes to
                // WrittingOver.
                for (name, policy) in self.policies.get(&pid).unwrap() {
                    if policy == &SchedulePolicy::Timed {
                        self.writer
                            .update_or_insert_collector_state(
                                pid,
                                name.clone(),
                                CollectorState::UnWritting,
                            )
                            .await;
                    }
                }
                self.manager.stop_collect(pid, &self.args).await?;
            }
            SchedulerEvent::StartPendingCollector(name, pid) => {
                // NOTE: only handle signals from deferred collectors (GPU indicators).
                let policies = self.policies.get(&pid).unwrap();
                if self
                    .manager
                    .get_pid_map()
                    .get(&pid)
                    .unwrap_or(&vec![])
                    .iter()
                    .any(|(indicator_name, collector_name)| {
                        indicator_name == &indicator_name::IndicatorName::GPU
                            && *collector_name == name
                    })
                {
                    self.manager
                        .trigger_pending_collect(pid, &self.args, policies)
                        .await?;
                    for (name, policies) in policies {
                        if policies == &SchedulePolicy::SignalTriggered {
                            self.writer
                                .update_or_insert_collector_state(
                                    pid,
                                    name.clone(),
                                    CollectorState::Collecting,
                                )
                                .await;
                        }
                    }
                } else {
                    log::debug!(
                        "Ignoring StartPendingCollector for non-GPU collector: {:?} for pid: {}",
                        name,
                        pid
                    );
                }
            }
            SchedulerEvent::StopCollector(name, pid) => {
                // NOTE: only handle signals from deferred collectors (GPU indicators).
                if self
                    .manager
                    .get_pid_map()
                    .get(&pid)
                    .unwrap_or(&vec![])
                    .iter()
                    .any(|(indicator_name, collector_name)| {
                        indicator_name == &indicator_name::IndicatorName::GPU
                            && *collector_name == name
                    })
                {
                    let policies = self.policies.get(&pid).unwrap();
                    for (name, policies) in policies {
                        if policies == &SchedulePolicy::SignalTriggered {
                            self.writer
                                .update_or_insert_collector_state(
                                    pid,
                                    name.clone(),
                                    CollectorState::UnWritting,
                                )
                                .await;
                        }
                    }
                    self.manager.stop_collect(pid, &self.args).await?;
                } else {
                    log::warn!(
                        "Ignoring StopCollector for non-GPU collector: {:?} for pid: {}",
                        name,
                        pid
                    );
                }
            }
            SchedulerEvent::WritingFinish(name, pid) => {
                self.manager.shutdown(&name, pid).await?;
                self.writer
                    .update_or_insert_collector_state(pid, name, CollectorState::WrittingOver)
                    .await;
                self.writer
                    .write_json(pid, args, name, self.manager.get_metadata())
                    .await;
            }
            SchedulerEvent::CollectFailed(name, pid) => {
                // Guarded lookup: detector::manager seeds a pid entry for every
                // requested pid and collector_manager starts a unix socket
                // listener for each of them, but policies/writer state are only
                // created per detected collector. A pid whose indicators all
                // detected false therefore has a live socket and no entry here,
                // so an unsolicited failure message must not panic the whole
                // run (it is reachable now that CUPTIProfilingFailed is mapped
                // onto this event).
                let signal_triggered = match self.policies.get(&pid) {
                    Some(policies) => policies
                        .iter()
                        .any(|(_, v)| v == &SchedulePolicy::SignalTriggered),
                    None => {
                        log::warn!(
                            "Ignoring CollectFailed for pid {}: no scheduled collectors",
                            pid
                        );
                        return Ok(true);
                    }
                };

                if self.args.duration != 0 {
                    if signal_triggered {
                        let sender = EventHandler::sender();
                        sender.send(SchedulerEvent::StartPendingCollector(name, pid))?;
                        self.start_auto_stop_timer(
                            Duration::from_secs(self.args.duration as u64),
                            pid,
                        );
                    }

                    self.writer
                        .handle_collect_failed(pid, name, false, args)
                        .await;
                } else {
                    self.writer
                        .handle_collect_failed(pid, name, true, args)
                        .await;
                }
                self.manager.shutdown(&name, pid).await?;
            }
            SchedulerEvent::Exit => {
                self.manager.drop().await;
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub async fn start_event_loop(&mut self, args: &ProfileArgs) -> Result<()> {
        info!("Starting scheduler event loop");

        // Watchdog: the loop normally exits on SchedulerEvent::Exit, emitted
        // once every collector reaches WrittingOver. If a collector crashes
        // without reporting (e.g. pyki's injector aborts the target), that
        // event never comes and recv() would block forever — wedging the
        // whole process and, upstream, the client's TaskRunner. Bound the
        // wait to duration + slack so a stalled run always terminates.
        let idle_budget = Duration::from_secs(self.args.duration as u64 + 60);

        // The channel is std::sync::mpsc: recv_timeout is a blocking syscall.
        // Calling it directly from this async fn parks a tokio worker for the
        // whole wait window. Under a small worker pool (docker CPU quota=1),
        // that starves the runtime — auto_stop_timer's sleep never wakes,
        // injector tasks are never polled, and the collection silently
        // returns no data. Move each blocking wait onto tokio's blocking
        // thread pool so worker threads stay free to drive timers and
        // spawn_blocking-based injectors.
        loop {
            let recv_result = tokio::task::spawn_blocking(move || {
                let receiver = EventHandler::receiver().lock().unwrap();
                receiver.recv_timeout(idle_budget)
            })
            .await;

            let event = match recv_result {
                Ok(Ok(event)) => event,
                Ok(Err(std::sync::mpsc::RecvTimeoutError::Timeout)) => {
                    error!(
                        "Scheduler idle for {:?} with no events (likely a crashed collector); \
                         forcing shutdown",
                        idle_budget
                    );
                    // Salvage whatever did finish. Since StopTimedCollector no
                    // longer pre-stamps WrittingOver, a collector that dies
                    // without reporting leaves its pid below WrittingOver
                    // forever, so is_all_pid_finish never fires and the
                    // merge/upload step would be skipped entirely — losing the
                    // healthy pids along with the broken one.
                    self.writer.finalize_pending_as_failed(args).await;
                    self.manager.drop().await;
                    break;
                }
                Ok(Err(e)) => {
                    error!("Event receiver error: {}", e);
                    break;
                }
                Err(e) => {
                    error!("recv join error: {}", e);
                    break;
                }
            };

            match self.handle_event(event, args).await {
                Ok(true) => continue,
                Ok(false) => {
                    info!("Exiting scheduler event loop");
                    break;
                }
                Err(e) => {
                    error!("Error handling event: {}", e);
                }
            }
        }
        Ok(())
    }

    fn start_auto_stop_timer(&self, duration: Duration, pid: i32) {
        let sender = EventHandler::global_sender();
        tokio::spawn(async move {
            info!(
                "Starting auto-stop timer for collector, duration: {:?}",
                duration
            );
            sleep(duration).await;
            if let Err(e) = sender.send(SchedulerEvent::StopTimedCollector(pid)) {
                error!("Failed to send StopCollector event: {}", e);
            }
        });
    }
}
