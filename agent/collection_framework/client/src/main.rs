// Entry point: parse CLI/env config, wire logging, build a TaskRunner
// with an Uploader (dashrs client), then loop-reconnect the control-plane
// session forever with 1s→60s exponential backoff.
//
// The transport is chosen by the TRANSPORT env var: `ws` (default) or
// `poll` for deployments behind proxies that strip the WS Upgrade header.
//
// All heavy lifting lives in the sibling modules — this file is the
// composition root and nothing else.

mod config;
mod gpu_procs;
mod poll;
mod profiler;
mod protocol;
mod task_runner;
mod uploader;
mod ws;

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tokio::time;
use tracing as log;
use tracing_subscriber::{
    filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
};

use crate::config::{Cli, Config};
use crate::task_runner::TaskRunner;
use crate::uploader::Uploader;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Arc::new(Config::from_cli(cli));

    // Anything other than `poll` keeps the historical WebSocket behaviour.
    let use_poll = std::env::var("TRANSPORT")
        .map(|v| v.eq_ignore_ascii_case("poll"))
        .unwrap_or(false);

    set_logger(cfg.verbose, cfg.log_dir.as_deref());
    let (transport, url) = if use_poll {
        ("poll", &cfg.http_url)
    } else {
        ("ws", &cfg.ws_url)
    };
    log::info!(
        client_id = %cfg.client_id,
        transport = transport,
        url = %url,
        "profiling-client starting"
    );

    let uploader = Uploader::new(&cfg)?;
    let runner = TaskRunner::new(uploader);

    let mut backoff = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);
    loop {
        let session = if use_poll {
            poll::run_session(Arc::clone(&cfg), Arc::clone(&runner)).await
        } else {
            ws::run_session(Arc::clone(&cfg), Arc::clone(&runner)).await
        };
        match session {
            Ok(_) => {
                log::info!("session closed cleanly, reconnecting immediately");
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                log::warn!(error = %e, backoff_secs = backoff.as_secs(), "session error, backing off");
                time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

fn set_logger(verbose: u8, log_dir: Option<&str>) {
    let default_level = match verbose {
        0 => LevelFilter::WARN,
        1 => LevelFilter::INFO,
        2 => LevelFilter::DEBUG,
        _ => LevelFilter::TRACE,
    };
    let filter = EnvFilter::builder()
        .with_default_directive(default_level.into())
        .from_env_lossy();

    let stdout_layer = fmt::layer().with_writer(std::io::stdout);

    if let Some(dir) = log_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("warn: failed to create log dir {}: {}", dir, e);
        }
        let now = chrono::Local::now();
        let path = format!("{}/profiling-client-{}.log", dir, now.format("%Y-%m-%d"));
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                let file = Arc::new(std::sync::Mutex::new(file));
                let file_layer = fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || -> Box<dyn Write + Send> {
                        struct FileWriter(Arc<std::sync::Mutex<std::fs::File>>);
                        impl Write for FileWriter {
                            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                                self.0.lock().unwrap().write(buf)
                            }
                            fn flush(&mut self) -> std::io::Result<()> {
                                self.0.lock().unwrap().flush()
                            }
                        }
                        Box::new(FileWriter(Arc::clone(&file)))
                    });
                let _ = tracing_subscriber::registry()
                    .with(stdout_layer)
                    .with(file_layer)
                    .with(filter)
                    .try_init();
                return;
            }
            Err(e) => {
                eprintln!("warn: failed to open log file {}: {} — stdout only", path, e);
            }
        }
    }

    let _ = tracing_subscriber::registry()
        .with(stdout_layer)
        .with(filter)
        .try_init();
}
