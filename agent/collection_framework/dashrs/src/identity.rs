// ClientIdentity — resolve the fields the AIProf dashboardServer.js
// REGISTER handler expects, with a CLI → env → hostname fallback chain.
//
// The dashboard server (see `dashboardServer.js:236-248`) is lenient:
// only `clientId` is required; `namespace/podName/nodeName/labels` can be
// empty strings and the server backfills from any existing record (or
// leaves them blank on a first sighting).  So we only hard-fail when we
// cannot produce a non-empty `client_id`.

use std::collections::HashMap;

use crate::error::{DashError, Result};

/// Everything the collector needs to associate an upload with a client.
#[derive(Debug, Clone)]
pub struct ClientIdentity {
    /// Non-empty. REGISTER key in the dashboard server.
    pub client_id: String,
    /// Kubernetes namespace. May be empty on non-k8s hosts.
    pub namespace: String,
    /// Kubernetes pod name. May be empty on non-k8s hosts.
    pub pod_name: String,
    /// Kubernetes node name. May be empty on non-k8s hosts.
    pub node_name: String,
    /// Optional client labels (dashboard treats these as pass-through metadata).
    pub labels: HashMap<String, String>,
    /// Optional feature tags — dashboard does not enforce.
    pub capabilities: Vec<String>,
}

impl ClientIdentity {
    /// Multi-layer resolve:
    ///   * `client_id`  = CLI → `CLIENT_ID` env → hostname → hard-error
    ///   * `namespace`  = CLI → `NAMESPACE` env → `POD_NAMESPACE` env → ""
    ///   * `pod_name`   = CLI → `POD_NAME` env → ""
    ///   * `node_name`  = CLI → `NODE_NAME` env → ""
    pub fn resolve(
        cli_client_id: Option<&str>,
        cli_namespace: Option<&str>,
        cli_pod: Option<&str>,
        cli_node: Option<&str>,
    ) -> Result<Self> {
        let client_id = pick(cli_client_id, &["CLIENT_ID"])
            .or_else(gethostname_string)
            .ok_or_else(|| {
                DashError::MissingIdentity(
                    "client_id: cannot resolve via --client-id, $CLIENT_ID, or gethostname"
                        .into(),
                )
            })?;
        if client_id.trim().is_empty() {
            return Err(DashError::MissingIdentity(
                "client_id resolved to an empty string".into(),
            ));
        }
        let namespace = pick(cli_namespace, &["NAMESPACE", "POD_NAMESPACE"]).unwrap_or_default();
        let pod_name = pick(cli_pod, &["POD_NAME"]).unwrap_or_default();
        let node_name = pick(cli_node, &["NODE_NAME"]).unwrap_or_default();
        Ok(Self {
            client_id,
            namespace,
            pod_name,
            node_name,
            labels: HashMap::new(),
            capabilities: Vec::new(),
        })
    }

    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        self.labels = labels;
        self
    }

    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }
}

fn pick(cli: Option<&str>, env_names: &[&str]) -> Option<String> {
    if let Some(v) = cli {
        let t = v.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    for name in env_names {
        if let Ok(v) = std::env::var(name) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn gethostname_string() -> Option<String> {
    // POSIX gethostname(3). 256 bytes is well above HOST_NAME_MAX on Linux (64).
    let mut buf = vec![0i8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr(), buf.len()) };
    if ret != 0 {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    let s = cstr.to_string_lossy().to_string();
    if s.trim().is_empty() { None } else { Some(s) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env-mutating tests must run serially inside this file.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_env() {
        for k in ["CLIENT_ID", "NAMESPACE", "POD_NAMESPACE", "POD_NAME", "NODE_NAME"] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn cli_overrides_env_and_hostname() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("CLIENT_ID", "from-env");
        let id = ClientIdentity::resolve(Some("from-cli"), None, None, None).unwrap();
        assert_eq!(id.client_id, "from-cli");
        clear_env();
    }

    #[test]
    fn env_overrides_hostname() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("CLIENT_ID", "env-host");
        std::env::set_var("NAMESPACE", "team-a");
        std::env::set_var("POD_NAME", "cf-abcd");
        std::env::set_var("NODE_NAME", "node-42");
        let id = ClientIdentity::resolve(None, None, None, None).unwrap();
        assert_eq!(id.client_id, "env-host");
        assert_eq!(id.namespace, "team-a");
        assert_eq!(id.pod_name, "cf-abcd");
        assert_eq!(id.node_name, "node-42");
        clear_env();
    }

    #[test]
    fn hostname_fallback_when_no_cli_or_env() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        let id = ClientIdentity::resolve(None, None, None, None).unwrap();
        // gethostname should return something non-empty on any real host.
        assert!(!id.client_id.is_empty());
        // Namespace/pod/node fall back to empty strings.
        assert_eq!(id.namespace, "");
        assert_eq!(id.pod_name, "");
        assert_eq!(id.node_name, "");
    }

    #[test]
    fn cli_empty_string_is_ignored() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("CLIENT_ID", "env-host");
        let id = ClientIdentity::resolve(Some("   "), None, None, None).unwrap();
        assert_eq!(id.client_id, "env-host");
        clear_env();
    }

    #[test]
    fn pod_namespace_env_is_alias_for_namespace() {
        let _g = ENV_LOCK.lock().unwrap();
        clear_env();
        std::env::set_var("CLIENT_ID", "h");
        std::env::set_var("POD_NAMESPACE", "kube-system");
        let id = ClientIdentity::resolve(None, None, None, None).unwrap();
        assert_eq!(id.namespace, "kube-system");
        clear_env();
    }
}
