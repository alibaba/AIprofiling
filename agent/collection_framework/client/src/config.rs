// Runtime configuration for the profiling client agent.
//
// Precedence: CLI flag → environment variable → sensible default (or
// hostname for `client_id`, matching `dashrs::ClientIdentity::resolve`).

use std::env;

use clap::Parser;

const DEFAULT_SERVER_HOST: &str = "localhost:7000";
const DEFAULT_HEARTBEAT_SECS: u64 = 30;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "AIProf profiling client agent")]
pub struct Cli {
    /// dashboardServer host, `host:port` (env: SERVER_HOST).
    #[arg(long, value_name = "HOST")]
    pub server_host: Option<String>,

    /// clientId reported in REGISTER (env: CLIENT_ID, falls back to hostname).
    #[arg(long, value_name = "ID")]
    pub client_id: Option<String>,

    /// Kubernetes namespace (env: NAMESPACE or POD_NAMESPACE).
    #[arg(long, value_name = "NS")]
    pub namespace: Option<String>,

    /// Kubernetes pod name (env: POD_NAME).
    #[arg(long, value_name = "POD")]
    pub pod_name: Option<String>,

    /// Kubernetes node name (env: NODE_NAME).
    #[arg(long, value_name = "NODE")]
    pub node_name: Option<String>,

    /// Seconds between HEARTBEAT frames (env: HEARTBEAT_SECS, default 30).
    #[arg(long, value_name = "SECS")]
    pub heartbeat_secs: Option<u64>,

    /// Optional log directory. When omitted, logs go only to stdout.
    #[arg(long, value_name = "DIR")]
    pub log_dir: Option<String>,

    /// Log verbosity: 0=warn 1=info 2=debug 3=trace. Overridden by RUST_LOG.
    #[arg(long, default_value = "1")]
    pub verbose: u8,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub ws_url: String,
    pub http_url: String,
    pub client_id: String,
    pub namespace: String,
    pub pod_name: String,
    pub node_name: String,
    pub heartbeat_secs: u64,
    pub log_dir: Option<String>,
    pub verbose: u8,
}

impl Config {
    pub fn from_cli(cli: Cli) -> Self {
        let server_host = cli
            .server_host
            .or_else(|| env::var("SERVER_HOST").ok())
            .unwrap_or_else(|| DEFAULT_SERVER_HOST.to_string());

        let client_id = cli
            .client_id
            .or_else(|| env::var("CLIENT_ID").ok())
            .unwrap_or_else(|| {
                hostname::get()
                    .map(|h| h.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "unknown-client".to_string())
            });

        let namespace = cli
            .namespace
            .or_else(|| env::var("NAMESPACE").ok())
            .or_else(|| env::var("POD_NAMESPACE").ok())
            .unwrap_or_default();

        let pod_name = cli
            .pod_name
            .or_else(|| env::var("POD_NAME").ok())
            .unwrap_or_default();

        let node_name = cli
            .node_name
            .or_else(|| env::var("NODE_NAME").ok())
            .unwrap_or_default();

        let heartbeat_secs = cli
            .heartbeat_secs
            .or_else(|| env::var("HEARTBEAT_SECS").ok().and_then(|s| s.parse().ok()))
            .unwrap_or(DEFAULT_HEARTBEAT_SECS);

        let log_dir = cli.log_dir.or_else(|| env::var("LOG_DIR").ok());

        // SERVER_HOST may carry an explicit scheme (`wss://host:443`,
        // `https://host`) to talk to a TLS-terminating reverse proxy. A bare
        // `host:port` keeps the historical plaintext behaviour. When the
        // scheme is TLS the WS path may already be embedded (e.g. behind an
        // nginx `location /aiprof/`), so honour a path if one is present and
        // fall back to `/ws` otherwise.
        let (ws_url, http_url) = build_urls(&server_host);

        Config {
            ws_url,
            http_url,
            client_id,
            namespace,
            pod_name,
            node_name,
            heartbeat_secs,
            log_dir,
            verbose: cli.verbose,
        }
    }
}

// Derive the WebSocket control URL and HTTP upload base from SERVER_HOST.
//
// Accepted forms:
//   host:port                  → ws://host:port/ws   + http://host:port
//   ws://host:port             → ws://host:port/ws   + http://host:port
//   wss://host[:port]          → wss://host[:port]/ws + https://host[:port]
//   https://host[:port]        → wss://host[:port]/ws + https://host[:port]
//   wss://host/prefix          → wss://host/prefix/ws + https://host/prefix
//   https://host/prefix        → wss://host/prefix/ws + https://host/prefix
//
// The `/prefix` form is for reverse proxies that mount the server under a
// path (e.g. nginx `location /aiprof/`). The prefix is preserved on both
// URLs, so uploads land at `https://host/prefix/api/results/upload` and the
// proxy strips the prefix before forwarding to the server.
fn build_urls(server_host: &str) -> (String, String) {
    let (scheme, rest) = match server_host.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        None => ("ws".to_string(), server_host),
    };
    let tls = scheme == "wss" || scheme == "https";
    let (authority, prefix) = match rest.split_once('/') {
        Some((a, p)) => {
            let trimmed = p.trim_end_matches('/');
            if trimmed.is_empty() {
                (a, String::new())
            } else {
                (a, format!("/{}", trimmed))
            }
        }
        None => (rest, String::new()),
    };
    let ws_scheme = if tls { "wss" } else { "ws" };
    let http_scheme = if tls { "https" } else { "http" };
    (
        format!("{}://{}{}/ws", ws_scheme, authority, prefix),
        format!("{}://{}{}", http_scheme, authority, prefix),
    )
}

#[cfg(test)]
mod tests {
    use super::build_urls;

    #[test]
    fn legacy_host_port_stays_plaintext() {
        let (ws, http) = build_urls("localhost:7000");
        assert_eq!(ws, "ws://localhost:7000/ws");
        assert_eq!(http, "http://localhost:7000");
    }

    #[test]
    fn wss_scheme_upgrades_both() {
        let (ws, http) = build_urls("wss://example.com:443");
        assert_eq!(ws, "wss://example.com:443/ws");
        assert_eq!(http, "https://example.com:443");
    }

    #[test]
    fn https_scheme_maps_to_wss() {
        let (ws, http) = build_urls("https://example.com");
        assert_eq!(ws, "wss://example.com/ws");
        assert_eq!(http, "https://example.com");
    }

    #[test]
    fn tls_with_path_prefix_is_honoured() {
        let (ws, http) = build_urls("wss://example.com/aiprof");
        assert_eq!(ws, "wss://example.com/aiprof/ws");
        assert_eq!(http, "https://example.com/aiprof");
    }

    #[test]
    fn tls_with_trailing_slash_is_normalised() {
        let (ws, http) = build_urls("https://example.com/aiprof/");
        assert_eq!(ws, "wss://example.com/aiprof/ws");
        assert_eq!(http, "https://example.com/aiprof");
    }

    #[test]
    fn explicit_ws_scheme_is_plaintext() {
        let (ws, http) = build_urls("ws://10.0.0.1:7000");
        assert_eq!(ws, "ws://10.0.0.1:7000/ws");
        assert_eq!(http, "http://10.0.0.1:7000");
    }
}
