// src/detector/manager.rs
use crate::collector::collector_name::CollectorName;
use crate::collector::indicator_name::IndicatorName;
use crate::command::ProfileArgs;
use crate::config::{Config, DependencyConfig};
use crate::error::ErrorCode;
use crate::meta::Meta;
use anyhow::Result;
use log::{debug, info, warn};
use std::collections::HashMap;
use tracing as log;

// DetectorFn signature: returns the list of PIDs that satisfy the dependency.
// The extra meta argument lets each detector record diagnostic info per-PID.
type DetectorFn = fn(&DependencyConfig, &ProfileArgs, &mut HashMap<i32, Meta>) -> Result<Vec<i32>>;

// Holds a cached detector result.
#[derive(Debug, Clone)]
struct CachedDetectionResult {
    pids: Vec<i32>,
}

pub struct DetectorManager {
    detectors: HashMap<String, DetectorFn>,
    // Caches detector results
    cache: HashMap<String, CachedDetectionResult>,
}

impl DetectorManager {
    pub fn new() -> Self {
        let mut manager = DetectorManager {
            detectors: HashMap::new(),
            cache: HashMap::new(),
        };

        manager.register_detector(
            "detect_nvidia_or_ppu",
            crate::detector::detectors::detect_nvidia_or_ppu,
        );

        manager.register_detector(
            "detect_nvidia",
            crate::detector::detectors::detect_nvidia_or_ppu,
        );

        manager.register_detector(
            "detect_python_support",
            crate::detector::detectors::detect_python_support,
        );

        manager.register_detector(
            "detect_torch_support",
            crate::detector::detectors::detect_torch_support,
        );

        manager.register_detector(
            "detect_cuda_support",
            crate::detector::detectors::detect_cuda_support,
        );

        manager.register_detector(
            "detect_nvidia_driver",
            crate::detector::detectors::detect_nvidia_driver,
        );

        manager.register_detector(
            "detect_os_support",
            crate::detector::detectors::detect_os_support,
        );

        manager.register_detector("detect_inject", crate::detector::detectors::detect_inject);

        manager.register_detector(
            "detect_pip_support",
            crate::detector::detectors::detect_pip_support,
        );

        manager
    }

    fn register_detector(&mut self, name: &str, detector: DetectorFn) {
        self.detectors.insert(name.to_string(), detector);
    }

    /// Returns the list of enabled collector indicators per PID.
    pub fn run_all_detections(
        &mut self,
        config: &Config,
        args: &ProfileArgs,
        meta: &mut HashMap<i32, Meta>,
    ) -> HashMap<i32, Vec<(IndicatorName, CollectorName)>> {
        // Clear the cache so every run performs fresh detection.
        self.cache.clear();

        let mut result: HashMap<i32, Vec<(IndicatorName, CollectorName)>> =
            args.pids.iter().map(|&pid| (pid, Vec::new())).collect();

        info!("Running all detections...");

        // Iterate over every supported indicator to see which need to be enabled.
        for indicator in &config.indicators {
            debug!("indicator name is {}", indicator.name);
            let indicator_enabled = match indicator.name.as_str() {
                "GPU" => args.gpu.unwrap_or(false),
                "PyStack" => args.pystack.unwrap_or(false),
                "Torch" => args.torch.unwrap_or(false),
                "Snapshot" => args.snapshot.unwrap_or(false),
                _ => false,
            };

            // Only run dependency detection when the collector is enabled via CLI flags.
            if indicator_enabled {
                let indicator_name = IndicatorName::new(&indicator.name);
                for collector in &indicator.collectors {
                    let collector_name = CollectorName::new(&collector.name);
                    if collector_name == CollectorName::Unknown {
                        warn!("Collector '{}' is unknown. Skipping...", collector.name);
                        continue;
                    }

                    // For each collector, check all of its dependencies.
                    // Only add the collector when every dependency is satisfied.
                    let mut all_pids: Option<Vec<i32>> = None;
                    let mut all_dependencies_satisfied = true;

                    for dependency in &collector.dependencies {
                        match self.run_dependency_detection(dependency, &args, meta) {
                            Ok(pids) => {
                                if pids.is_empty() {
                                    // If any dependency has no satisfying PID, the collector fails as a whole.
                                    all_dependencies_satisfied = false;
                                    break;
                                } else {
                                    // Intersect the PIDs satisfying every dependency.
                                    match &mut all_pids {
                                        None => {
                                            all_pids = Some(pids);
                                        }
                                        Some(current_pids) => {
                                            current_pids.retain(|pid| pids.contains(pid));
                                            if current_pids.is_empty() {
                                                all_dependencies_satisfied = false;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                warn!(
                                    "Dependency detection failed for '{}': {:?}",
                                    dependency.detector, err
                                );
                                all_dependencies_satisfied = false;
                                break;
                            }
                        }
                    }

                    // Only add the collector when all dependencies are satisfied.
                    if all_dependencies_satisfied && all_pids.is_some() {
                        let pids = all_pids.unwrap();
                        if !pids.is_empty() {
                            for pid in pids {
                                result
                                    .entry(pid)
                                    .or_default()
                                    .push((indicator_name, collector_name));
                            }
                        }
                        log::debug!(
                            "Collector '{}' for indicator '{}' will be enabled.",
                            collector.name,
                            indicator.name,
                        );
                        break;
                    }
                }
            }
        }

        result
    }

    /// Returns the list of PIDs that satisfy this collector's prerequisites.
    fn run_dependency_detection(
        &mut self,
        dependency: &DependencyConfig,
        args: &ProfileArgs,
        meta: &mut HashMap<i32, Meta>,
    ) -> Result<Vec<i32>> {
        // Build a unique key from the detector name plus its sorted params.
        // Sorting params ensures identical configs yield identical keys.
        let mut param_items: Vec<(&String, &serde_json::Value)> =
            dependency.params.iter().collect();
        param_items.sort_by_key(|&(key, _)| key);
        let cache_key = format!(
            "{}_{}",
            dependency.detector,
            serde_json::to_string(&param_items).unwrap_or_else(|_| "".to_string())
        );

        // Check for a cached result.
        if let Some(cached_result) = self.cache.get(&cache_key) {
            return Ok(cached_result.pids.clone());
        }

        // Otherwise, run the detector.
        if let Some(detector_fn) = self.detectors.get(&dependency.detector) {
            match detector_fn(dependency, args, meta) {
                Ok(pids) => {
                    debug!(
                        "Processes matching dependency '{}': {:?}",
                        dependency.detector, pids
                    );

                    // Cache the result.
                    self.cache
                        .insert(cache_key, CachedDetectionResult { pids: pids.clone() });

                    Ok(pids)
                }
                Err(e) => {
                    warn!(
                        "Dependency '{}' detection failed: {:?}",
                        dependency.detector, e
                    );
                    Err(e)
                }
            }
        } else {
            Err(ErrorCode::NotFoundDetector(Some(format!(
                "Detector function '{}' not found",
                dependency.detector
            )))
            .into_error())
        }
    }
}
