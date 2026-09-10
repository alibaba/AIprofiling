use crate::plugins::plugin_adapter::{PluginConfig, PluginFunction};
use serde::Serialize;
use tracing as log;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize)]
pub enum CollectorName {
    Pyki,
    CUPTI,

    Unknown,
}

impl CollectorName {
    pub fn new(name: &str) -> Self {
        log::debug!("CollectorName::new: {}", name);
        let binding = name.to_lowercase();
        let name = binding.as_str();

        match name {
            "pyki" => CollectorName::Pyki,
            "cupti" => CollectorName::CUPTI,
            &_ => CollectorName::Unknown,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            CollectorName::Pyki => "Pyki",
            CollectorName::CUPTI => "CUPTI",
            CollectorName::Unknown => "Unknown",
        }
    }

    pub fn create_plugin_config(&self, path: String) -> PluginConfig {
        match self {
            CollectorName::Pyki => PluginConfig {
                path,
                function_mapping: vec![
                    (PluginFunction::Init, None),
                    (PluginFunction::Shutdown, None),
                    (PluginFunction::Trigger, None),
                    (PluginFunction::Stop, None),
                ]
                .into_iter()
                .collect(),
                name: CollectorName::Pyki,
            },
            CollectorName::CUPTI => PluginConfig {
                path,
                function_mapping: vec![
                    (PluginFunction::Init, None),
                    (PluginFunction::Shutdown, None),
                    (PluginFunction::Trigger, None),
                    (PluginFunction::Stop, None),
                ]
                .into_iter()
                .collect(),
                name: CollectorName::CUPTI,
            },
            CollectorName::Unknown => unimplemented!(),
        }
    }
}
