// src/meta.rs
use crate::collector::collector_name::CollectorName;
use crate::collector::indicator_name::IndicatorName;
use crate::tools::chrome_time::ChromeTraceBaseTime;
use serde::Serialize;

// per pid
#[derive(Serialize)]
pub struct Meta {
    #[serde(rename = "deviceProperties")]
    pub device_properties: Vec<DeviceProperties>, //TODO: if a process uses multiple GPUs, we need to record info for all of them
    #[serde(rename = "baseTimeNanoseconds")]
    pub base_time_nanoseconds: i64,
    pub indicators: Vec<IndicatorState>,
    pub message: String, // used to check whether the process exists and whether its resources meet the requirements
    #[serde(rename = "statStep")]
    pub stat_step: Vec<StepInfo>,
    #[serde(rename = "stepEndTime")]
    pub step_end_time: Vec<i64>,
}

#[derive(Serialize)]
pub struct DeviceProperties {
    pub name: String, // GPU name
    pub id: i32,      // GPU id
    #[serde(rename = "totalGlobalMem")]
    pub total_global_mem: u64, // GPU total global memory
    #[serde(rename = "memoryUsed")]
    pub memory_used: u64, // GPU memory used
}

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct StepInfo {
    pub id: String,
    pub start: i64,
    pub dur: i64,
    pub loss: String,
}

// If an indicator collection fails, we need to record the failure reason
#[derive(Serialize)]
pub struct IndicatorState {
    pub indicator_name: IndicatorName,
    pub collector_name: CollectorName,
    pub state: String,
    pub message: String, // for display
}

impl Meta {
    pub fn new() -> Self {
        Meta {
            device_properties: vec![], // no longer add an empty device property by default
            base_time_nanoseconds: 0,
            indicators: vec![],
            message: "".to_string(),
            stat_step: vec![],
            step_end_time: vec![],
        }
    }

    pub fn registor_indicator(
        &mut self,
        indicator_name: &IndicatorName,
        collector_name: &CollectorName,
        state: &str,
        message: &str,
    ) {
        let indicator_state = IndicatorState::new(indicator_name, collector_name, state, message);
        self.indicators.push(indicator_state);
    }

    pub fn registor_device_properties(
        &mut self,
        name: &str,
        id: i32,
        total_global_mem: u64,
        memory_used: u64,
    ) {
        self.device_properties
            .push(DeviceProperties::new_with_memory(
                name,
                id,
                total_global_mem,
                memory_used,
            ));
    }

    pub fn update_or_register_device_properties(
        &mut self,
        name: &str,
        id: i32,
        total_global_mem: u64,
        memory_used: u64,
    ) {
        // Check whether the same device property already exists
        let mut exists = false;
        for dp in &self.device_properties {
            if dp.name == name
                && dp.id == id
                && dp.total_global_mem == total_global_mem
                && dp.memory_used == memory_used
            {
                exists = true;
                break;
            }
        }

        // Only add if no identical entry exists
        if !exists {
            self.registor_device_properties(name, id, total_global_mem, memory_used);
        }
    }

    pub fn registor_basetime(&mut self) {
        self.base_time_nanoseconds = ChromeTraceBaseTime::get_base_time();
    }

    pub fn add_step_info(&mut self, step: StepInfo) {
        self.stat_step.push(step);
    }

    pub fn add_step_end_time(&mut self, time: i64) {
        self.step_end_time.push(time);
    }
}

impl IndicatorState {
    pub fn new(
        indicator_name: &IndicatorName,
        collector_name: &CollectorName,
        state: &str,
        message: &str,
    ) -> Self {
        IndicatorState {
            indicator_name: indicator_name.clone(),
            collector_name: *collector_name,
            state: state.to_string(),
            message: message.to_string(),
        }
    }
}

impl DeviceProperties {
    pub fn new_with_memory(name: &str, id: i32, total_global_mem: u64, memory_used: u64) -> Self {
        DeviceProperties {
            name: name.to_string(),
            id: id,
            total_global_mem,
            memory_used,
        }
    }
}
