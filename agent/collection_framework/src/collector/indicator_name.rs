// /src/collextor/indicator_name.rs
use tracing as log;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub enum IndicatorName {
    GPU,
    Torch,
    PyStack,
    Snapshot,

    Unknown,
}

impl IndicatorName {
    pub fn new(name: &str) -> Self {
        log::debug!("new indicator name: {}", name);
        match name {
            "GPU" => IndicatorName::GPU,
            "Torch" => IndicatorName::Torch,
            "PyStack" => IndicatorName::PyStack,
            "Snapshot" => IndicatorName::Snapshot,
            &_ => IndicatorName::Unknown,
        }
    }
}
