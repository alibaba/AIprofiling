// src/config.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct DependencyConfig {
    pub detector: String,
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CollectorItem {
    pub name: String,
    pub path: String,
    pub config_path: String,
    pub dependencies: Vec<DependencyConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IndicatorItem {
    pub name: String,
    pub collectors: Vec<CollectorItem>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ResourceItem {
    pub name: String,
    pub dependencies: Vec<DependencyConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub resources: Vec<ResourceItem>,
    pub indicators: Vec<IndicatorItem>,
}

impl Config {
    pub fn from_path<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let file = fs::read_to_string(path)?;
        let config = serde_yaml::from_str(&file)?;
        Ok(config)
    }

    pub fn get_collector_config(&self, name: &str) -> Option<&CollectorItem> {
        for item in &self.indicators {
            for collector in &item.collectors {
                if collector.name == name {
                    return Some(collector);
                }
            }
        }
        None
    }
}
