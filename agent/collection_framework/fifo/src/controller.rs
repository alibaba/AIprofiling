use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};

use super::{log, receiver, worker, FifoHandlerCommand, FifoHandlerInfo, GenericResult};

pub(crate) fn controller(
    output: PathBuf,
    info: broadcast::Sender<FifoHandlerInfo>,
    mut cmd: mpsc::UnboundedReceiver<FifoHandlerCommand>,
) -> GenericResult {
    loop {
        match cmd.blocking_recv() {
            Some(FifoHandlerCommand::Listen((
                id,
                fifo_name,
                base_path,
                limit,
                legacy_output,
                pid_maps,
            ))) => {
                let output = output.clone();
                let info = info.clone();
                let (upstream_tx, mut upstream_rx) = mpsc::unbounded_channel();
                let receiver_name = fifo_name.clone();
                std::thread::spawn(move || {
                    receiver::upstream_receiver(
                        output,
                        &mut upstream_rx,
                        info,
                        receiver_name,
                        id,
                        limit,
                        legacy_output,
                        pid_maps,
                    )
                });
                std::thread::spawn(move || {
                    worker::listen_worker_sync(upstream_tx, base_path.clone(), fifo_name, id)
                });
            }
            None => {
                log::info!("handle droped");
                break;
            }
        }
    }
    Ok(())
}
