// plugin_adapter.rs

use crate::collector::collector_name::CollectorName;
use crate::plugins::cupti_plugin_wrapper::CUPTIPluginWrapper;
use crate::plugins::pyki_plugin_wrapper::PykiPluginWrapper;
use crate::ErrorCode;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub enum PluginFunction {
    Init,
    Shutdown,
    Trigger,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Ord, Eq, PartialOrd)]
pub enum CollectorState {
    Initializing = 0, // initializing
    InitSuccess = 1,  // init succeeded
    Collecting = 2,   // collecting
    ShuttingDown = 3, // writing complete, stopping

    UnWritting = 6,   // a pid has not started writing
    WrittingOver = 7, // a pid has finished writing
    Failed = 8,       // a pid failed to write
}

#[derive(Debug, Clone)]
pub struct PluginConfig {
    pub path: String,
    pub function_mapping: HashMap<PluginFunction, Option<String>>,
    pub name: CollectorName,
}

#[async_trait]
pub trait GenericPlugin: Send + Sync {
    async fn call_function(&mut self, func: PluginFunction, params: &Value) -> Result<Value>;
}

pub struct GenericPluginWrapper {
    plugin: Box<dyn GenericPlugin>,
}

impl GenericPluginWrapper {
    pub unsafe fn new(config: PluginConfig) -> Result<Self> {
        // Create the matching plugin instance based on the config
        let plugin = Self::create_plugin(&config)?;
        Ok(GenericPluginWrapper { plugin })
    }

    unsafe fn create_plugin(config: &PluginConfig) -> Result<Box<dyn GenericPlugin>> {
        match config.name {
            CollectorName::Pyki => Ok(Box::new(PykiPluginWrapper::new(config.clone())?)),
            CollectorName::CUPTI => Ok(Box::new(CUPTIPluginWrapper::new(config.clone())?)),

            _ => Err(ErrorCode::UnsupportedPlugin(None).into_error()),
        }
    }

    pub async fn init(&mut self, args: &Value) -> Result<()> {
        self.plugin
            .call_function(PluginFunction::Init, args)
            .await?;
        Ok(())
    }

    pub async fn shutdown(&mut self, args: &Value) -> Result<Value> {
        self.plugin
            .call_function(PluginFunction::Shutdown, args)
            .await
    }

    pub async fn trigger(&mut self, args: &Value) -> Result<()> {
        self.plugin
            .call_function(PluginFunction::Trigger, args)
            .await?;
        Ok(())
    }

    pub async fn stop(&mut self, pid: i32, dir_path: &str) -> Result<()> {
        let params = json!({
            "pid": pid,
            "dir_path": dir_path
        });
        self.plugin
            .call_function(PluginFunction::Stop, &params)
            .await?;
        Ok(())
    }
}

#[macro_export]
macro_rules! load_symbol {
    ($config:expr, $lib:expr, $symbols:expr, $func:ident, $ctor:ident, $fn_type:ty) => {
        if let Some(Some(name)) = $config.function_mapping.get(&PluginFunction::$func) {
            let symbol: Symbol<$fn_type> = unsafe { $lib.get(name.as_bytes())? };
            $symbols.insert(
                PluginFunction::$func,
                FunctionSymbol::$ctor(unsafe { std::mem::transmute(symbol) }),
            );
        }
    };
}
