// HTTP long-poll control plane — the fallback for `ws.rs`.
//
// Some reverse proxies strip `Connection: Upgrade` / `Upgrade: websocket`,
// which degrades the WS handshake into a plain GET and makes the WS control
// plane unusable. This module speaks the same four control messages over
// ordinary HTTP requests instead:
//
//   REGISTER     → POST {base}/api/agent/register
//   HEARTBEAT    → POST {base}/api/agent/heartbeat
//   NEW_TASK     ← GET  {base}/api/agent/poll?client_id=..&wait=..  (server holds)
//   TASK_RESULT  → POST {base}/api/agent/task_result
//
// Trace upload is unaffected: it already goes over HTTP via `uploader.rs`.
// Selected by the `TRANSPORT=poll` env var; `ws` remains the default.
//
// Like `ws::run_session`, this function does not reconnect on its own — the
// loop in `main` re-invokes it after any Ok or Err.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time;
use tracing as log;

use crate::config::Config;
use crate::protocol::{ClientMsg, ProfilingConfig};
use crate::task_runner::{BusyError, ResultSender, TaskRunner};

/// Seconds the server is asked to hold an idle poll open. The server caps
/// this at 60; staying below a typical proxy read timeout avoids 504s.
const POLL_WAIT_SECS: u64 = 30;
/// Client-side read timeout must exceed POLL_WAIT_SECS or every idle poll
/// would abort locally before the server's own timer fires.
const POLL_READ_TIMEOUT: Duration = Duration::from_secs(POLL_WAIT_SECS + 15);
const MAX_POLL_BACKOFF: Duration = Duration::from_secs(30);

pub async fn run_session(cfg: Arc<Config>, runner: Arc<TaskRunner>) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(POLL_READ_TIMEOUT)
        .build()
        .context("build poll http client")?;
    let base = cfg.http_url.clone();

    // A failed REGISTER propagates so `main`'s backoff loop retries the
    // whole session rather than polling against a server that never saw us.
    register(&http, &base, &cfg).await?;
    log::info!(client_id = %cfg.client_id, base = %base, "REGISTER sent (poll transport)");
    post_heartbeat(&http, &base, &cfg, &runner).await?;

    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<ClientMsg>();

    let mut hb = time::interval(Duration::from_secs(cfg.heartbeat_secs));
    hb.tick().await; // discard the immediate first tick

    let mut poll_backoff = Duration::from_secs(1);
    let mut poll_fut = Box::pin(poll_once(&http, &base, &cfg.client_id));

    loop {
        tokio::select! {
            _ = hb.tick() => {
                // A failed heartbeat is not fatal: the poll GET also refreshes
                // the server's liveness clock, so one lost beat is harmless.
                if let Err(e) = post_heartbeat(&http, &base, &cfg, &runner).await {
                    log::warn!(error = %e, "heartbeat POST failed");
                }
            }
            res = &mut poll_fut => {
                match res {
                    Ok(Some((analysis_id, profiling_config))) => {
                        poll_backoff = Duration::from_secs(1);
                        dispatch_task(&cfg, &runner, analysis_id, profiling_config, &result_tx).await;
                    }
                    Ok(None) => {
                        // The server's wait elapsed with nothing queued: re-poll at once.
                        poll_backoff = Duration::from_secs(1);
                    }
                    Err(e) => {
                        log::warn!(error = %e, backoff_secs = poll_backoff.as_secs(), "poll GET failed");
                        time::sleep(poll_backoff).await;
                        poll_backoff = (poll_backoff * 2).min(MAX_POLL_BACKOFF);
                    }
                }
                poll_fut = Box::pin(poll_once(&http, &base, &cfg.client_id));
            }
            Some(out) = result_rx.recv() => {
                if let Err(e) = post_task_result(&http, &base, &cfg.client_id, &out).await {
                    log::warn!(error = %e, "task_result POST failed");
                }
            }
        }
    }
}

async fn register(http: &reqwest::Client, base: &str, cfg: &Config) -> Result<()> {
    let body = json!({
        "clientId": cfg.client_id,
        "namespace": cfg.namespace,
        "podName": cfg.pod_name,
        "nodeName": cfg.node_name,
        "labels": HashMap::<String, String>::new(),
        "capabilities": ["gpu-profiling"],
        "version": env!("CARGO_PKG_VERSION"),
    });
    let url = format!("{}/api/agent/register", base);
    http.post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {}", url))?
        .error_for_status()
        .with_context(|| format!("POST {}", url))?;
    Ok(())
}

async fn post_heartbeat(
    http: &reqwest::Client,
    base: &str,
    cfg: &Config,
    runner: &TaskRunner,
) -> Result<()> {
    let running = runner.running_task_ids().await;
    let status = if running.is_empty() { "Idle" } else { "Profiling" };
    let gpu_procs = crate::gpu_procs::list_gpu_procs()
        .await
        .unwrap_or_default();
    let body = json!({
        "clientId": cfg.client_id,
        "status": status,
        "runningTaskIds": running,
        "gpuProcs": gpu_procs,
        "ts": Utc::now().timestamp(),
    });
    let url = format!("{}/api/agent/heartbeat", base);
    http.post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {}", url))?
        .error_for_status()
        .with_context(|| format!("POST {}", url))?;
    Ok(())
}

async fn post_task_result(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
    msg: &ClientMsg,
) -> Result<()> {
    // The runner emits TaskResult; other ClientMsg variants (e.g. the WS-only
    // ListGpuProcsResult) have no poll endpoint and are dropped.
    let ClientMsg::TaskResult { analysis_id, status, message } = msg else {
        log::debug!("ignoring non-TaskResult on the poll transport");
        return Ok(());
    };
    let body = json!({
        "clientId": client_id,
        "analysisId": analysis_id,
        "status": status,
        "message": message,
    });
    let url = format!("{}/api/agent/task_result", base);
    http.post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {}", url))?
        .error_for_status()
        .with_context(|| format!("POST {}", url))?;
    log::info!(analysis_id = %analysis_id, "TASK_RESULT posted");
    Ok(())
}

async fn poll_once(
    http: &reqwest::Client,
    base: &str,
    client_id: &str,
) -> Result<Option<(String, ProfilingConfig)>> {
    let url = format!(
        "{}/api/agent/poll?client_id={}&wait={}",
        base,
        urlencode(client_id),
        POLL_WAIT_SECS
    );
    let text = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {}", url))?
        .error_for_status()
        .with_context(|| format!("GET {}", url))?
        .text()
        .await
        .context("read poll body")?;
    parse_poll_response(&text)
}

/// `{ "task": { "analysisId", "profilingConfig" } }` → Some, `{ "task": null }` → None.
fn parse_poll_response(text: &str) -> Result<Option<(String, ProfilingConfig)>> {
    #[derive(serde::Deserialize)]
    struct PollBody {
        #[serde(default)]
        task: Option<PollTask>,
    }
    #[derive(serde::Deserialize)]
    struct PollTask {
        #[serde(rename = "analysisId")]
        analysis_id: String,
        #[serde(rename = "profilingConfig", default)]
        profiling_config: ProfilingConfig,
    }
    let body: PollBody = serde_json::from_str(text).context("parse poll response")?;
    Ok(body
        .task
        .map(|t| (t.analysis_id, t.profiling_config)))
}

/// clientId goes into a query string; percent-encode everything outside the
/// unreserved set rather than pulling in a URL-encoding crate for one call.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// Mirrors the ws.rs NEW_TASK arm: admit the task, or report Failed when the
// runner is already busy so the server does not wait out its watchdog.
async fn dispatch_task(
    cfg: &Arc<Config>,
    runner: &Arc<TaskRunner>,
    analysis_id: String,
    profiling_config: ProfilingConfig,
    result_tx: &ResultSender,
) {
    log::info!(analysis_id = %analysis_id, "task received (poll)");
    match Arc::clone(runner)
        .try_run(
            analysis_id.clone(),
            profiling_config,
            Arc::clone(cfg),
            result_tx.clone(),
        )
        .await
    {
        Ok(()) => {}
        Err(BusyError::Duplicate { .. }) => {
            log::info!(analysis_id = %analysis_id, "duplicate task ignored");
        }
        Err(BusyError::Busy { running }) => {
            log::warn!(
                analysis_id = %analysis_id,
                already_running = %running,
                "rejecting task — client is busy"
            );
            let _ = result_tx.send(ClientMsg::TaskResult {
                analysis_id,
                status: crate::protocol::TaskStatus::Failed,
                message: Some(format!("busy running {}", running)),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_task_with_config() {
        let raw = r#"{"task":{"analysisId":"uuid-1","profilingConfig":{"timeout":3000,"pids":"12,34"}}}"#;
        let (id, cfg) = parse_poll_response(raw).unwrap().expect("a task");
        assert_eq!(id, "uuid-1");
        assert_eq!(cfg.timeout, Some(3000));
        assert_eq!(cfg.pids.to_vec(), vec!["12", "34"]);
    }

    #[test]
    fn parse_null_task_is_none() {
        assert!(parse_poll_response(r#"{"task":null}"#).unwrap().is_none());
    }

    #[test]
    fn parse_missing_task_key_is_none() {
        assert!(parse_poll_response("{}").unwrap().is_none());
    }

    #[test]
    fn parse_task_without_config_uses_defaults() {
        let (id, cfg) = parse_poll_response(r#"{"task":{"analysisId":"x"}}"#)
            .unwrap()
            .expect("a task");
        assert_eq!(id, "x");
        assert_eq!(cfg.timeout, None);
        assert!(cfg.pids.to_vec().is_empty());
    }

    #[test]
    fn parse_garbage_is_an_error() {
        assert!(parse_poll_response("not json").is_err());
    }

    #[test]
    fn urlencode_passes_through_safe_chars() {
        assert_eq!(urlencode("aiprof-client-120"), "aiprof-client-120");
    }

    #[test]
    fn urlencode_escapes_separators() {
        assert_eq!(urlencode("a b&c=d"), "a%20b%26c%3Dd");
    }
}
