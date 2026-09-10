// Wire types for the WebSocket control channel with the AIProf
// `dashboardServer.js`.
//
// Contract source: `server/dashboard/dashboardServer.js` — the server
// switches on the JSON `type` field (SCREAMING_SNAKE_CASE), and every
// message we send has the client_id at top level so the server can look
// up the connection record.
//
// `ProfilingConfig` mirrors the shape the dashboard ships inside a
// NEW_TASK message. The server currently sends free-form JSON; we
// deserialize leniently (all optional) and rebuild the CollectionFramework
// CLI in `profiler::build_command`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientMsg {
    Register {
        #[serde(rename = "clientId")]
        client_id: String,
        namespace: String,
        #[serde(rename = "podName")]
        pod_name: String,
        #[serde(rename = "nodeName")]
        node_name: String,
        labels: HashMap<String, String>,
        capabilities: Vec<String>,
        version: String,
    },
    Heartbeat {
        #[serde(rename = "clientId")]
        client_id: String,
        status: ClientStatus,
        #[serde(rename = "runningTaskIds")]
        running_task_ids: Vec<String>,
        ts: i64,
    },
    TaskResult {
        #[serde(rename = "analysisId")]
        analysis_id: String,
        status: TaskStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    ListGpuProcsResult {
        #[serde(rename = "reqId")]
        req_id: String,
        procs: Vec<GpuProc>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// One row from `nvidia-smi --query-compute-apps=pid,process_name,used_gpu_memory`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GpuProc {
    pub pid: u32,
    pub name: String,
    #[serde(rename = "memMiB")]
    pub mem_mib: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServerMsg {
    Registered {
        #[serde(rename = "clientId", default)]
        client_id: String,
    },
    HeartbeatAck,
    NewTask {
        #[serde(rename = "analysisId")]
        analysis_id: String,
        #[serde(rename = "profilingConfig", default)]
        profiling_config: ProfilingConfig,
    },
    Error {
        #[serde(default)]
        message: String,
    },
    ListGpuProcs {
        #[serde(rename = "reqId")]
        req_id: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum ClientStatus {
    Idle,
    Profiling,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum TaskStatus {
    Succeeded,
    Failed,
}

/// The `profilingConfig` object inside a NEW_TASK. Every field is
/// optional because the dashboard frontend does not always populate
/// them (e.g. duration mode omits `iteration`, and vice versa).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfilingConfig {
    /// Duration in milliseconds. Divided by 1000 for `--duration`.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Iteration range `[start, end]`. Presence flips `--iteration`.
    #[serde(default)]
    pub iteration: Option<Vec<u64>>,
    #[serde(default)]
    pub iteration_module: Option<String>,
    #[serde(default)]
    pub iteration_function: Option<String>,
    /// Aliases used by some frontend versions.
    #[serde(default)]
    pub iteration_mod: Option<String>,
    #[serde(default)]
    pub iteration_func: Option<String>,
    /// Analysis toggles: adapt/snapshot/python/kernel/pytorch.
    /// See `profiler::apply_analysis_params`.
    #[serde(default)]
    pub analysis_params: Vec<String>,
    /// PIDs to attach. Comes across the wire as a CSV string
    /// (`"1234,5678"`); accept a JSON array too.
    #[serde(default)]
    pub pids: PidList,
    #[serde(default)]
    pub comms: Option<String>,
}

impl ProfilingConfig {
    // `comms` is accepted from the wire but currently unused when
    // building the CollectionFramework CLI. Kept in the struct so we
    // do not silently drop it if the frontend starts populating it.
    #[allow(dead_code)]
    pub fn comms(&self) -> Option<&str> {
        self.comms.as_deref()
    }
}

/// Accept either a CSV string or a JSON array of integers.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(untagged)]
pub enum PidList {
    #[default]
    Empty,
    Csv(String),
    Vec(Vec<i32>),
}

impl PidList {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            PidList::Empty => Vec::new(),
            PidList::Csv(s) => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
            PidList::Vec(v) => v.iter().map(|p| p.to_string()).collect(),
        }
    }
}

impl ProfilingConfig {
    pub fn iteration_range(&self) -> Option<(u64, u64)> {
        let r = self.iteration.as_ref()?;
        let start = r.first().copied().unwrap_or(0);
        let end = r.get(1).copied().unwrap_or(0);
        Some((start, end))
    }

    /// The frontend uses `iteration_mod`/`iteration_func` in some
    /// paths and the underscored variants in others.
    pub fn effective_iteration_module(&self) -> Option<&str> {
        self.iteration_module
            .as_deref()
            .or(self.iteration_mod.as_deref())
            .filter(|s| !s.is_empty())
    }

    pub fn effective_iteration_function(&self) -> Option<&str> {
        self.iteration_function
            .as_deref()
            .or(self.iteration_func.as_deref())
            .filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_serializes_with_expected_keys() {
        let msg = ClientMsg::Register {
            client_id: "host-1".into(),
            namespace: "ns".into(),
            pod_name: "pod".into(),
            node_name: "node".into(),
            labels: HashMap::new(),
            capabilities: vec!["gpu-profiling".into()],
            version: "0.1.0".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "REGISTER");
        assert_eq!(v["clientId"], "host-1");
        assert_eq!(v["podName"], "pod");
        assert_eq!(v["nodeName"], "node");
        assert_eq!(v["capabilities"][0], "gpu-profiling");
    }

    #[test]
    fn heartbeat_serializes_status_and_task_ids() {
        let msg = ClientMsg::Heartbeat {
            client_id: "host-1".into(),
            status: ClientStatus::Profiling,
            running_task_ids: vec!["abc".into()],
            ts: 1_700_000_000,
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "HEARTBEAT");
        assert_eq!(v["status"], "Profiling");
        assert_eq!(v["runningTaskIds"][0], "abc");
        assert_eq!(v["ts"], 1_700_000_000);
    }

    #[test]
    fn task_result_omits_none_message() {
        let msg = ClientMsg::TaskResult {
            analysis_id: "abc".into(),
            status: TaskStatus::Succeeded,
            message: None,
        };
        let v: serde_json::Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["type"], "TASK_RESULT");
        assert_eq!(v["status"], "Succeeded");
        assert!(v.get("message").is_none());
    }

    #[test]
    fn deserialize_new_task_full() {
        let raw = r#"{
            "type": "NEW_TASK",
            "analysisId": "uuid-1",
            "profilingConfig": {
                "timeout": 3000,
                "analysis_params": ["adapt", "kernel"],
                "pids": "1234,5678"
            }
        }"#;
        let m: ServerMsg = serde_json::from_str(raw).unwrap();
        match m {
            ServerMsg::NewTask { analysis_id, profiling_config } => {
                assert_eq!(analysis_id, "uuid-1");
                assert_eq!(profiling_config.timeout, Some(3000));
                assert_eq!(profiling_config.pids.to_vec(), vec!["1234", "5678"]);
                assert_eq!(profiling_config.analysis_params.len(), 2);
            }
            other => panic!("expected NewTask, got {:?}", other),
        }
    }

    #[test]
    fn deserialize_pids_as_json_array() {
        let raw = r#"{"type":"NEW_TASK","analysisId":"x","profilingConfig":{"pids":[1,2,3]}}"#;
        let m: ServerMsg = serde_json::from_str(raw).unwrap();
        if let ServerMsg::NewTask { profiling_config, .. } = m {
            assert_eq!(profiling_config.pids.to_vec(), vec!["1", "2", "3"]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn deserialize_iteration_and_aliases() {
        let raw = r#"{
            "type":"NEW_TASK",
            "analysisId":"x",
            "profilingConfig": {"iteration":[2,7],"iteration_mod":"m","iteration_func":"f"}
        }"#;
        let m: ServerMsg = serde_json::from_str(raw).unwrap();
        if let ServerMsg::NewTask { profiling_config, .. } = m {
            assert_eq!(profiling_config.iteration_range(), Some((2, 7)));
            assert_eq!(profiling_config.effective_iteration_module(), Some("m"));
            assert_eq!(profiling_config.effective_iteration_function(), Some("f"));
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn deserialize_registered_and_ack() {
        let r: ServerMsg = serde_json::from_str(r#"{"type":"REGISTERED","clientId":"h"}"#).unwrap();
        assert!(matches!(r, ServerMsg::Registered { .. }));
        let a: ServerMsg = serde_json::from_str(r#"{"type":"HEARTBEAT_ACK"}"#).unwrap();
        assert!(matches!(a, ServerMsg::HeartbeatAck));
    }
}
