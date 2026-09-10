use crate::collector::collector_name::CollectorName;
use crate::collector::indicator_name::IndicatorName;
use crate::collector::scheduler::SchedulePolicy;
use crate::command::ProfileArgs;
use crate::config::Config;
use crate::detector::manager::DetectorManager;
use crate::error::ErrorCode;
use crate::meta::{Meta, StepInfo};
use crate::plugins::plugin_adapter::GenericPluginWrapper;
use crate::plugins::plugin_adapter::CollectorState;
use crate::tools::writer::Writer;
use crate::tools::gpu_process::{
    convert_pid_to_gpu_id_nvidia, detect_gpu_type,
};
use crate::tools::gpu::GpuType;
use crate::unix_handler::UnixSocketHandler;
use anyhow::Result;
use log::{debug, error, info};
use std::collections::{HashMap, HashSet};
use tokio::sync::broadcast;
use tracing as log;

pub struct CollectorManager {
    // Collector handles
    collectors: HashMap<CollectorName, GenericPluginWrapper>,

    // Collectors enabled for each pid
    pid_map: HashMap<i32, Vec<(IndicatorName, CollectorName)>>,

    // Unix socket listener task handles
    unixsock_map: HashMap<i32, Option<tokio::task::JoinHandle<()>>>,

    // Sender used to shut down unix socket listener tasks
    shutdown_sender: Option<broadcast::Sender<()>>,

    meta: HashMap<i32, Meta>,
}

impl CollectorManager {
    /*
     *1. Start all collectors
     *2. Detect which indicators each process can enable
     */
    pub unsafe fn new(config: &Config, args: &mut ProfileArgs) -> Result<Self> {
        /// Filter out invalid processes
        fn filter_valid_pids(
            mut config: Config,
            meta: &mut HashMap<i32, Meta>,
            args: &ProfileArgs,
        ) -> Vec<i32> {
            // Compute mem_threshold: base_factor * pid_count * duration or iteration
            let pid_count = args.pids.len() as f32;

            // Read the MEM resource threshold from config.yaml as base_factor, default 0.5
            let mut base_factor: f32 = 0.5;
            'outer: for resource in &config.resources {
                if resource.name == "MEM" {
                    for dependency in &resource.dependencies {
                        if dependency.detector == "detect_mem_support" {
                            if let Some(value) = dependency
                                .params
                                .get("threshold")
                                .and_then(|v| v.as_str())
                            {
                                // Threshold looks like "< 0.5" or "0.5"; keep only the number
                                let trimmed = value.trim();
                                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                                let number_str = if parts.len() == 1 {
                                    parts[0]
                                } else {
                                    parts[parts.len() - 1]
                                };
                                if let Ok(v) = number_str.parse::<f32>() {
                                    base_factor = v;
                                }
                            }
                            break 'outer;
                        }
                    }
                }
            }

            let threshold_value = if args.iteration {
                // iteration mode
                base_factor * pid_count * (args.num_steps as f32)
            } else {
                // duration mode
                base_factor * pid_count * (args.duration as f32)
            };

            // Update mem_threshold in config
            // Locate the MEM resource and update its dependency
            let mut updated = false;
            let mut resource_index = 0;
            let mut dependency_index = 0;

            // Find the resource and dependency indices to update
            for (i, resource) in config.resources.iter().enumerate() {
                if resource.name == "MEM" {
                    for (j, dependency) in resource.dependencies.iter().enumerate() {
                        if dependency.detector == "detect_mem_support" {
                            resource_index = i;
                            dependency_index = j;
                            updated = true;
                            break;
                        }
                    }
                    if updated {
                        break;
                    }
                }
            }

            // Apply the update if a matching entry was found
            if updated {
                config.resources[resource_index].dependencies[dependency_index]
                    .params
                    .insert(
                        "threshold".to_string(),
                        serde_json::Value::String(format!("< {:.2}", threshold_value)),
                    );
            }

            let mut valid_pids = Vec::new();
            for pid in &args.pids {
                // Check the PID: process must exist and have enough resources
                if crate::tools::utils::is_pid_valid(&config, *pid) {
                    valid_pids.push(*pid);
                } else {
                    // Process missing or resources insufficient; record in meta.message
                    let meta_entry = meta.entry(*pid).or_insert_with(Meta::new);
                    if !crate::tools::utils::process_exists(*pid) {
                        meta_entry.message = format!("Process {} does not exist", pid);
                    } else {
                        meta_entry.message =
                            format!("Process {} does not meet resource requirements", pid);
                    }
                    log::error!("Skipping process {}", pid);
                }
            }
            valid_pids
        }

        /// Decide which indicators to enable per pid: those supported by the
        /// current environment/process intersected with those requested by the user.
        fn enable_collectors(
            config: Config,
            args: ProfileArgs,
            meta: &mut HashMap<i32, Meta>,
        ) -> Result<HashMap<i32, Vec<(IndicatorName, CollectorName)>>> {
            let mut detections = DetectorManager::new();

            if let Some(gpu_type) = detect_gpu_type() {
                match gpu_type {
                    GpuType::Nvidia => {
                        let _ = convert_pid_to_gpu_id_nvidia(&args.pids);
                    }
                    _ => {
                        // do nothing: only PPU cards support DCGM indicators
                    }
                }
            }

            // Refresh the GPU process cache before running detectors so they see current data
            if detect_gpu_type().is_some() {
                if let Err(e) = crate::tools::gpu_process::refresh_gpu_process_cache() {
                    log::warn!("Failed to refresh GPU process cache: {}", e);
                }
            }

            // Indicators supported by the current environment/process
            let pid_map = detections.run_all_detections(&config, &args, meta);

            Ok(pid_map)
        }

        // All collector names currently supported
        let mut need_enable_collectors = HashSet::new();
        let mut collectors: HashMap<CollectorName, GenericPluginWrapper> = HashMap::new();

        let mut meta = HashMap::new();
        // Mutable clone of config, used to update mem_threshold
        let mutable_config = config.clone();
        args.pids = filter_valid_pids(mutable_config, &mut meta, args);
        if args.pids.is_empty() {
            let msg = format!("No processes meet the resource requirements, exiting...");
            error!(msg);
            return Err(ErrorCode::DetectEnvError(None)
                .with_details(msg)
                .into_error());
        }

        // Clone config and args for enable_collectors since it takes ownership
        let detect_config = config.clone();
        let detect_args = args.clone();
        let pid_map = enable_collectors(detect_config, detect_args, &mut meta)?;

        info!("Detected collectors: {:?}", pid_map);

        for (_, names) in &pid_map {
            for (_, name) in names {
                need_enable_collectors.insert(name.clone());
            }
        }

        // Instantiate the required collectors
        for name in &need_enable_collectors {
            'outer: for iter in &config.indicators {
                for collector in &iter.collectors {
                    // Case-insensitive comparison
                    if collector.name.to_lowercase() == name.name().to_lowercase() {
                        let path = collector.path.clone();
                        let collector_config = name.create_plugin_config(path);
                        let collector = GenericPluginWrapper::new(collector_config)?;
                        collectors.insert(name.clone(), collector);
                        break 'outer;
                    }
                }
            }
        }

        // Set baseTimeNanoseconds on each PID's meta
        for pid in &args.pids {
            if let Some(meta_entry) = meta.get_mut(pid) {
                meta_entry.registor_basetime();
            } else {
                // No meta entry yet for this pid; create one and set baseTimeNanoseconds
                let mut meta_entry = Meta::new();
                meta_entry.registor_basetime();
                meta.insert(*pid, meta_entry);
            }
        }

        Ok(Self {
            collectors: collectors,
            pid_map: pid_map,
            unixsock_map: HashMap::new(),
            shutdown_sender: None,
            meta,
        })
    }

    fn init_event_thread(&mut self, pid: i32) {
        // Create or get the broadcast channel sender
        let shutdown_sender = match &self.shutdown_sender {
            Some(sender) => sender.clone(),
            None => {
                // Create a new broadcast channel with buffer size 16
                let (sender, _) = broadcast::channel(16);
                self.shutdown_sender = Some(sender.clone());
                sender
            }
        };

        let handle: tokio::task::JoinHandle<()> =
            UnixSocketHandler::start_listen(pid, shutdown_sender);
        self.unixsock_map.insert(pid, Some(handle));
    }

    // Helper: write collector state into meta
    fn update_collector_status_in_meta(
        &mut self,
        pid: i32,
        indicator_name: &IndicatorName,
        collector_name: &CollectorName,
        state: &str,
        message: &str,
    ) {
        if let Some(meta_entry) = self.meta.get_mut(&pid) {
            meta_entry.registor_indicator(indicator_name, collector_name, state, message);
        }
    }

    pub async fn init_collectors(&mut self, params: &ProfileArgs, _config: &Config, writer: &mut Writer) -> Result<()> {
        let pids: Vec<_> = self.pid_map.keys().cloned().collect();

        // Initialize event handling threads
        for pid in &pids {
            self.init_event_thread(*pid);
        }

        // Track per-PID collector init status
        let mut pid_init_status = HashMap::new();

        // Initialize collectors
        for pid in &pids {
            let indicator_collect = self.pid_map.get(pid).unwrap();
            let grouped: HashMap<CollectorName, Vec<IndicatorName>> =
                indicator_collect.iter().fold(
                    HashMap::new(),
                    |mut acc, (indicator_name, collector_name)| {
                        acc.entry(collector_name.clone())
                            .or_insert_with(Vec::new)
                            .push(indicator_name.clone());
                        acc
                    },
                );

            let total_collectors = grouped.len();
            let mut failed_collectors = Vec::new();

            for (name, indicaters) in grouped {
                debug!("Initializing collector: {:?}", name);
                writer.update_or_insert_collector_state(*pid, name, CollectorState::Initializing).await;
                let result = match name {
                    CollectorName::Pyki => {
                        let mut params_clone = params.clone();
                        params_clone.target_pid = *pid;
                        let value = serde_json::to_value(&params_clone)?;
                        let collector = self.collectors.get_mut(&name).unwrap();
                        collector.init(&value).await
                    }
                    CollectorName::CUPTI => {
                        let mut params_clone = params.clone();
                        params_clone.target_pid = *pid;
                        let value = serde_json::to_value(&params_clone)?;
                        let collector = self.collectors.get_mut(&name).unwrap();
                        collector.init(&value).await
                    }
                    _ => {
                        let msg = format!("Unsupported collector: {:?}", name);
                        log::error!(msg);
                        Err(ErrorCode::UnsupportedPlugin(Some(msg)).into_error())
                    }
                };

                // On init failure, record it in meta
                if let Err(ref e) = result {
                    // Walk indicators, find those matching this collector, and mark failed
                    for indicator_name in &indicaters {
                        // Grab the pid_map reference first to avoid a second borrow of self in the closure
                        let pid_map_contains_indicator =
                            self.pid_map.get(pid).map_or(false, |indicators| {
                                indicators
                                    .iter()
                                    .any(|(ind, col)| *ind == *indicator_name && *col == name)
                            });

                        if pid_map_contains_indicator {
                            self.update_collector_status_in_meta(
                                *pid,
                                indicator_name,
                                &name,
                                "Failed",
                                &format!("Initialization failed: {}", e),
                            );
                        }
                    }

                    error!(
                        "Init collector {:?} failed for pid {}, error: {}",
                        name, pid, e
                    );
                    // Record failed collector, but don't return an error yet
                    failed_collectors.push(name.clone());
                }

                if result.is_err() {
                    // Already handled above; nothing to do here
                }
            }

            // Record init status for this PID
            pid_init_status.insert(*pid, (failed_collectors.len(), total_collectors));
        }

        // Check whether every collector failed for every PID
        let all_failed = !pid_init_status.is_empty()
            && pid_init_status
                .values()
                .all(|(failed, total)| *failed > 0 && failed == total);

        if all_failed {
            let msg = "All collectors failed to initialize for all PIDs!".to_string();
            error!(msg);
            return Err(ErrorCode::InitCollectorError(Some(msg)).into_error());
        } else {
            // Log which PIDs had some collectors fail
            for (pid, (failed, total)) in &pid_init_status {
                if *failed > 0 {
                    if failed == total {
                        error!("All collectors failed to initialize for PID {}", pid);
                    } else {
                        debug!(
                            "Some collectors failed to initialize for PID {}, {}/{} failed",
                            pid, failed, total
                        );
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn trigger_pending_collect(
        &mut self,
        pid: i32,
        params: &ProfileArgs,
        policies: &HashMap<CollectorName, SchedulePolicy>,
    ) -> Result<()> {
        for (collector_name, policy) in policies {
            if policy == &SchedulePolicy::SignalTriggered {
                self.trigger_collect(&collector_name, params, pid).await?;
            }
        }
        Ok(())
    }

    pub async fn trigger_collect(
        &mut self,
        name: &CollectorName,
        params: &ProfileArgs,
        pid: i32,
    ) -> Result<()> {
        debug!("PID:{} Start trigger {}", pid, name.name());
        let result = match name {
            CollectorName::Pyki => {
                let mut params_clone = params.clone();
                params_clone.target_pid = pid;

                let pid_map = self.get_pid_map().get(&pid).unwrap();
                let pyki_collector = CollectorName::Pyki;

                let indicators_to_check = [
                    IndicatorName::GPU,
                    IndicatorName::PyStack,
                    IndicatorName::Torch,
                    IndicatorName::Snapshot,
                ];

                for indicator in &indicators_to_check {
                    if pid_map
                        .iter()
                        .any(|(ind, collector)| *ind == *indicator && *collector == pyki_collector)
                    {
                        params_clone.enable_indicator.push(*indicator);
                    }
                }

                let value = serde_json::to_value(&params_clone)?;
                self.collectors.get_mut(name).unwrap().trigger(&value).await
            }
            CollectorName::CUPTI => {
                let mut params_clone = params.clone();
                params_clone.target_pid = pid;

                let value = serde_json::to_value(&params_clone)?;
                self.collectors.get_mut(name).unwrap().trigger(&value).await
            }

            _ => Ok(()),
        };

        // On trigger failure, record it in meta
        if let Err(ref e) = result {
            // Fetch the indicator list for this PID
            let pid_indicators: Vec<_> = self
                .pid_map
                .get(&pid)
                .map(|indicators| indicators.clone())
                .unwrap_or_default();

            // Walk indicators, find those matching this collector, and mark failed
            for (indicator_name, collector_name) in &pid_indicators {
                if collector_name == name {
                    self.update_collector_status_in_meta(
                        pid,
                        indicator_name,
                        name,
                        "Failed",
                        &format!("Trigger failed: {}", e),
                    );
                }
            }

            error!(
                "Trigger collector {:?} failed for pid {}, error: {}",
                name, pid, e
            );
        }

        // Return the error if trigger failed
        if let Err(e) = result {
            return Err(e);
        }

        debug!("PID:{} Trigger {} Successfully", pid, name.name());
        Ok(())
    }

    pub async fn stop_collect(&mut self, pid: i32, _params: &ProfileArgs) -> Result<()> {
        // Clone collector names to stop, to avoid borrow conflicts
        debug!("PID:{:#?} Stoping PID", self.pid_map);
        let binding = Vec::new();
        let names = self.pid_map.get(&pid).unwrap_or(&binding).clone();

        // Stop each collector
        for (indicator_name, name) in names {
            let result = match name {
                CollectorName::Pyki => self.collectors.get_mut(&name).unwrap().stop(pid, "").await,
                CollectorName::CUPTI => self.collectors.get_mut(&name).unwrap().stop(pid, "").await,
                _ => Ok(()),
            };

            // On stop failure, record it in meta
            if let Err(ref e) = result {
                // Record the stop failure in meta
                self.update_collector_status_in_meta(
                    pid,
                    &indicator_name,
                    &name,
                    "Failed",
                    &format!("Stop failed: {}", e),
                );

                error!(
                    "Stop collector {:?} failed for pid {}, error: {}",
                    name, pid, e
                );
            }
        }

        Ok(())
    }

    pub async fn shutdown(&mut self, name: &CollectorName, pid: i32) -> Result<()> {
        let result = match name {
            CollectorName::Pyki => {
                let value = serde_json::to_value(pid)?;
                let result = self
                    .collectors
                    .get_mut(name)
                    .unwrap()
                    .shutdown(&value)
                    .await;

                // Try to extract step data from shutdown's return value
                if let Ok(ref return_value) = result {
                    if let Ok(step_data) = serde_json::from_value::<Option<(Vec<StepInfo>, Vec<i64>)>>(
                        return_value.clone(),
                    ) {
                        if let Some((stat_step, step_end_time)) = step_data {
                            // Add step data to meta
                            if let Some(meta_entry) = self.meta.get_mut(&pid) {
                                for step in stat_step {
                                    meta_entry.add_step_info(step);
                                }
                                for time in step_end_time {
                                    meta_entry.add_step_end_time(time);
                                }
                                debug!("Added step data to meta for PID: {}", pid);
                            }
                        }
                    }
                }
                result.map(|_| ())
            }
            CollectorName::CUPTI => {
                let value = serde_json::to_value(pid)?;
                self.collectors
                    .get_mut(name)
                    .unwrap()
                    .shutdown(&value)
                    .await
                    .map(|_| ())
            }
            _ => Ok(()),
        };

        // Record shutdown result in meta
        // Fetch the indicator list for this PID
        let pid_indicators: Vec<_> = self
            .pid_map
            .get(&pid)
            .map(|indicators| indicators.clone())
            .unwrap_or_default();

        // Collect updates first to avoid a second mutable borrow of meta while it's held
        let mut updates = Vec::new();
        if let Some(meta_entry) = self.meta.get_mut(&pid) {
            // Walk indicators, find those matching this collector
            for (indicator_name, collector_name) in &pid_indicators {
                if collector_name == name {
                    // Was this collector already marked failed?
                    let is_already_failed = meta_entry.indicators.iter().any(|indicator_state| {
                        indicator_state.indicator_name == *indicator_name
                            && indicator_state.collector_name == *name
                            && indicator_state.state == "Failed"
                    });

                    if let Err(ref e) = result {
                        // Shutdown failed; record the error
                        updates.push((
                            indicator_name.clone(),
                            "Failed",
                            format!("Shutdown failed: {}", e),
                        ));
                    } else if !is_already_failed {
                        // Shutdown succeeded and wasn't previously failed
                        updates.push((indicator_name.clone(), "Success", String::new()));
                    }
                }
            }
        } else {
            error!("No meta entry found for pid: {}", pid);
        }

        // Apply status updates
        for (indicator_name, state, message) in updates {
            self.update_collector_status_in_meta(pid, &indicator_name, name, &state, &message);
        }

        // Log the outcome
        if let Err(ref e) = result {
            error!(
                "Shutdown collector {:?} failed for pid {}, error: {}",
                name, pid, e
            );
        }

        // Return the error if shutdown failed
        if let Err(e) = result {
            return Err(e);
        }

        Ok(())
    }

    pub fn get_pid_map(&self) -> &HashMap<i32, Vec<(IndicatorName, CollectorName)>> {
        &self.pid_map
    }

    pub fn get_metadata(&self) -> &HashMap<i32, Meta> {
        &self.meta
    }

    pub async fn drop(&mut self) {
        // Broadcast shutdown to all tasks
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        log::info!("CollectorManager dropping, cleaning up unix socket handlers...");

        for (_, handle_opt) in self.unixsock_map.iter_mut() {
            if let Some(handle) = handle_opt.take() {
                // Note: we can't await inside drop, so we abort synchronously here.
                // A truly async drop path would need to be refactored.
                handle.abort();
            }
        }
        log::info!("CollectorManager dropping, cleaning up unix socket handlers done.");
    }
}
