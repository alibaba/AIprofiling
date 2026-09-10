// One WebSocket session: connect → REGISTER → loop over
// (server frames | heartbeat ticks | outbound TaskResult).
//
// The reconnect loop in `main` re-invokes `run_session` after any
// Ok or Err — this function does not attempt its own reconnect.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing as log;

use crate::config::Config;
use crate::protocol::{ClientMsg, ClientStatus, ServerMsg};
use crate::task_runner::{BusyError, TaskRunner};

type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

pub async fn run_session(cfg: Arc<Config>, runner: Arc<TaskRunner>) -> Result<()> {
    let (ws_stream, _) = connect_async(&cfg.ws_url)
        .await
        .with_context(|| format!("ws connect {}", cfg.ws_url))?;
    let (mut write, mut read) = ws_stream.split();

    let register = ClientMsg::Register {
        client_id: cfg.client_id.clone(),
        namespace: cfg.namespace.clone(),
        pod_name: cfg.pod_name.clone(),
        node_name: cfg.node_name.clone(),
        labels: HashMap::new(),
        capabilities: vec!["gpu-profiling".into()],
        version: env!("CARGO_PKG_VERSION").into(),
    };
    send_client_msg(&mut write, &register).await?;
    log::info!(client_id = %cfg.client_id, "REGISTER sent");

    // Immediate first heartbeat so the server marks us live without
    // waiting a full interval.
    send_heartbeat(&mut write, &cfg, &runner).await?;

    let mut hb_interval = time::interval(std::time::Duration::from_secs(cfg.heartbeat_secs));
    hb_interval.tick().await; // discard the immediate first tick

    let (result_tx, mut result_rx) = mpsc::unbounded_channel::<ClientMsg>();

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_server_frame(&text, &cfg, &runner, &result_tx).await;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        log::info!(?frame, "server closed the WebSocket");
                        return Ok(());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => {
                        log::info!("WebSocket stream ended");
                        return Ok(());
                    }
                }
            }
            _ = hb_interval.tick() => {
                send_heartbeat(&mut write, &cfg, &runner).await?;
            }
            Some(out) = result_rx.recv() => {
                send_client_msg(&mut write, &out).await?;
            }
        }
    }
}

async fn send_client_msg(sink: &mut WsSink, msg: &ClientMsg) -> Result<()> {
    let text = serde_json::to_string(msg).context("serialize ClientMsg")?;
    sink.send(Message::Text(text)).await.context("ws send")?;
    Ok(())
}

async fn send_heartbeat(
    sink: &mut WsSink,
    cfg: &Config,
    runner: &TaskRunner,
) -> Result<()> {
    let running = runner.running_task_ids().await;
    let status = if running.is_empty() {
        ClientStatus::Idle
    } else {
        ClientStatus::Profiling
    };
    let hb = ClientMsg::Heartbeat {
        client_id: cfg.client_id.clone(),
        status,
        running_task_ids: running,
        ts: Utc::now().timestamp(),
    };
    send_client_msg(sink, &hb).await
}

async fn handle_server_frame(
    text: &str,
    cfg: &Arc<Config>,
    runner: &Arc<TaskRunner>,
    result_tx: &mpsc::UnboundedSender<ClientMsg>,
) {
    let parsed: ServerMsg = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(error = %e, raw = %text, "failed to parse server message");
            return;
        }
    };
    match parsed {
        ServerMsg::Registered { client_id } => {
            log::info!(client_id = %client_id, "REGISTERED");
        }
        ServerMsg::HeartbeatAck => {
            log::debug!("HEARTBEAT_ACK");
        }
        ServerMsg::NewTask { analysis_id, profiling_config } => {
            log::info!(analysis_id = %analysis_id, "NEW_TASK received");
            let runner = Arc::clone(runner);
            let cfg = Arc::clone(cfg);
            let tx = result_tx.clone();
            match runner
                .try_run(analysis_id.clone(), profiling_config, cfg, tx.clone())
                .await
            {
                Ok(()) => {}
                Err(BusyError::Duplicate { .. }) => {
                    log::info!(analysis_id = %analysis_id, "duplicate NEW_TASK ignored");
                }
                Err(BusyError::Busy { running }) => {
                    log::warn!(
                        analysis_id = %analysis_id,
                        already_running = %running,
                        "rejecting NEW_TASK — client is busy"
                    );
                    let _ = tx.send(ClientMsg::TaskResult {
                        analysis_id,
                        status: crate::protocol::TaskStatus::Failed,
                        message: Some(format!("busy running {}", running)),
                    });
                }
            }
        }
        ServerMsg::Error { message } => {
            log::error!(message = %message, "server ERROR");
        }
        ServerMsg::ListGpuProcs { req_id } => {
            log::info!(req_id = %req_id, "LIST_GPU_PROCS received");
            let tx = result_tx.clone();
            tokio::spawn(async move {
                let msg = match crate::gpu_procs::list_gpu_procs().await {
                    Ok(procs) => {
                        log::info!(req_id = %req_id, count = procs.len(), "LIST_GPU_PROCS_RESULT ok");
                        ClientMsg::ListGpuProcsResult { req_id, procs, error: None }
                    }
                    Err(e) => {
                        log::warn!(req_id = %req_id, error = %e, "LIST_GPU_PROCS failed");
                        ClientMsg::ListGpuProcsResult {
                            req_id,
                            procs: Vec::new(),
                            error: Some(e.to_string()),
                        }
                    }
                };
                let _ = tx.send(msg);
            });
        }
    }
}
