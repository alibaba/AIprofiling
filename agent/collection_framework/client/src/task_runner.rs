// Single-task runner: at most one CollectionFramework subprocess and
// one upload in flight. A second NEW_TASK for a different analysisId
// while busy is rejected up-front with a TASK_RESULT { Failed, "busy" }
// (matching the operator-selected "serial execution + busy rejection" mode).
//
// A repeat NEW_TASK for the *same* analysisId is treated as idempotent:
// silently acked, no second subprocess.

use std::sync::Arc;

use anyhow::Result;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};
use tracing as log;

use crate::config::Config;
use crate::profiler;
use crate::protocol::{ClientMsg, ProfilingConfig, TaskStatus};
use crate::uploader::Uploader;

#[derive(Debug, Error)]
pub enum BusyError {
    #[error("client is already running task {running}")]
    Busy { running: String },
    #[error("task {analysis_id} already running (idempotent)")]
    Duplicate { analysis_id: String },
}

pub type ResultSender = mpsc::UnboundedSender<ClientMsg>;

pub struct TaskRunner {
    /// `Some(analysis_id)` when a task is in flight.
    current: Mutex<Option<String>>,
    uploader: Uploader,
}

impl TaskRunner {
    pub fn new(uploader: Uploader) -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(None),
            uploader,
        })
    }

    /// Snapshot of currently running task IDs (0 or 1). Used by the
    /// HEARTBEAT builder to fill `runningTaskIds`.
    pub async fn running_task_ids(&self) -> Vec<String> {
        match &*self.current.lock().await {
            Some(id) => vec![id.clone()],
            None => Vec::new(),
        }
    }

    /// Try to admit and run a task. Returns immediately after spawning;
    /// the spawned future emits a TASK_RESULT via `tx` when finished.
    pub async fn try_run(
        self: Arc<Self>,
        analysis_id: String,
        cfg: ProfilingConfig,
        client_cfg: Arc<Config>,
        tx: ResultSender,
    ) -> Result<(), BusyError> {
        {
            let mut guard = self.current.lock().await;
            if let Some(running) = guard.as_ref() {
                if running == &analysis_id {
                    return Err(BusyError::Duplicate { analysis_id });
                }
                return Err(BusyError::Busy {
                    running: running.clone(),
                });
            }
            *guard = Some(analysis_id.clone());
        }

        let this = Arc::clone(&self);
        let aid = analysis_id.clone();
        tokio::spawn(async move {
            let result = this.execute(&aid, &cfg, &client_cfg).await;
            {
                let mut guard = this.current.lock().await;
                *guard = None;
            }
            let msg = match &result {
                Ok(()) => {
                    log::info!(analysis_id = %aid, "task finished successfully");
                    ClientMsg::TaskResult {
                        analysis_id: aid.clone(),
                        status: TaskStatus::Succeeded,
                        message: None,
                    }
                }
                Err(e) => {
                    log::error!(analysis_id = %aid, error = %e, "task failed");
                    ClientMsg::TaskResult {
                        analysis_id: aid.clone(),
                        status: TaskStatus::Failed,
                        message: Some(e.to_string()),
                    }
                }
            };
            let _ = tx.send(msg);
        });

        Ok(())
    }

    async fn execute(
        &self,
        analysis_id: &str,
        cfg: &ProfilingConfig,
        _client_cfg: &Config,
    ) -> Result<()> {
        let workdir = profiler::workdir_for(analysis_id);
        tokio::fs::create_dir_all(&workdir).await?;

        log::info!(analysis_id, workdir = ?workdir, "starting profiling");
        profiler::run(&workdir, cfg).await?;

        log::info!(analysis_id, "uploading result");
        self.uploader.upload(&workdir, analysis_id).await?;

        if let Err(e) = tokio::fs::remove_dir_all(&workdir).await {
            log::warn!(analysis_id, error = %e, ?workdir, "cleanup failed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Cli, Config};

    fn test_config() -> Arc<Config> {
        let cli = Cli {
            server_host: Some("127.0.0.1:1".into()),
            client_id: Some("test-host".into()),
            namespace: None,
            pod_name: None,
            node_name: None,
            heartbeat_secs: Some(30),
            log_dir: None,
            verbose: 0,
        };
        Arc::new(Config::from_cli(cli))
    }

    fn test_uploader() -> Uploader {
        Uploader::new(&test_config()).expect("build uploader")
    }

    #[tokio::test]
    async fn duplicate_analysis_id_is_rejected() {
        let runner = TaskRunner::new(test_uploader());
        // Fake a task in flight.
        *runner.current.lock().await = Some("aid".into());

        let (tx, _rx) = mpsc::unbounded_channel();
        let err = Arc::clone(&runner)
            .try_run(
                "aid".into(),
                ProfilingConfig::default(),
                test_config(),
                tx,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, BusyError::Duplicate { .. }));
    }

    #[tokio::test]
    async fn different_analysis_id_when_busy_is_rejected_as_busy() {
        let runner = TaskRunner::new(test_uploader());
        *runner.current.lock().await = Some("in-flight".into());

        let (tx, _rx) = mpsc::unbounded_channel();
        let err = Arc::clone(&runner)
            .try_run(
                "new".into(),
                ProfilingConfig::default(),
                test_config(),
                tx,
            )
            .await
            .unwrap_err();
        match err {
            BusyError::Busy { running } => assert_eq!(running, "in-flight"),
            other => panic!("expected Busy, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn running_task_ids_reflects_state() {
        let runner = TaskRunner::new(test_uploader());
        assert!(runner.running_task_ids().await.is_empty());
        *runner.current.lock().await = Some("x".into());
        assert_eq!(runner.running_task_ids().await, vec!["x"]);
    }
}
