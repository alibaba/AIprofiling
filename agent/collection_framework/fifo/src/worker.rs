use super::{log, GenericErr, GenericResult, UpstreamMessage};
use crate::{get_pid_ns, is_fifo, mkfifo, rmfifo, FIFO_BUFFER, READ_BUFFER, TILE_SIZE};
use nix::fcntl::{fcntl, FcntlArg};
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt as _},
};
use tokio::sync::mpsc;

struct FifoInstanceSync {
    pub buffer: std::io::BufReader<std::fs::File>,
    name: String,
    path: String,
    ns: usize,
}

impl FifoInstanceSync {
    pub fn new(base: &str, name: &str, pid: usize) -> Result<Self, GenericErr> {
        let path = format!("{}/{}", base, name);
        log::info!("establishing connection {}", name);
        mkfifo(&path)?;
        if is_fifo(&path)? {
            let fd = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(&path)?;
            if let Err(err) = fcntl(fd.as_raw_fd(), FcntlArg::F_SETPIPE_SZ(FIFO_BUFFER)) {
                log::warn!(
                    "fifo set pipe size error: {}, this may cause data loss",
                    err
                );
            }
            let buffer = std::io::BufReader::with_capacity(READ_BUFFER, fd);
            Ok(Self {
                buffer: buffer,
                name: name.to_string(),
                path: path,
                ns: get_pid_ns(pid).unwrap_or_default(),
            })
        } else {
            Err(GenericErr::from("invalid fifo file name"))
        }
    }
}

impl Drop for FifoInstanceSync {
    fn drop(&mut self) {
        log::info!("clearing {} ...", self.name);
        if let Err(err) = rmfifo(&self.path, Some(self.ns)) {
            log::warn!("delete {} error: {}", self.path, err);
        }
    }
}

pub fn listen_worker_sync(
    upstream: mpsc::UnboundedSender<UpstreamMessage>,
    base: String,
    name: String,
    id: usize,
) -> GenericResult {
    let mut inst = FifoInstanceSync::new(&base, &name, id)?;
    let name = &name;
    let mut count: usize = 0;
    let mut flag = false;
    if let Err(err) = upstream.send(UpstreamMessage::Up((id, name.to_string()))) {
        log::error!("{} send error: {}", name, err);
    }
    loop {
        let mut buf = [0; TILE_SIZE];
        match inst.buffer.read(&mut buf) {
            Ok(0) => {
                if flag {
                    log::info!("{} - {} EOF, Read: {}", id, name, count);
                    break;
                }
            }
            Ok(n) => {
                count += n;
                if !flag {
                    flag = true;
                }
                if let Err(err) =
                    upstream.send(UpstreamMessage::Buffer((id, name.to_string(), buf, n)))
                {
                    log::error!("{} send error: {}", name, err);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(err) => {
                log::error!("{} read error: {}", id, err);
                break;
            }
        }
    }
    if let Err(err) = upstream.send(UpstreamMessage::Finish((id, name.to_string()))) {
        log::error!("{} send error: {}", name, err);
    }
    log::info!("{} - {} done", id, name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receiver;
    use pretty_hex::*;
    use rand::{distr::Alphanumeric, Rng};
    use std::path::Path;
    use std::sync::Once;
    use tokio::sync::{broadcast, mpsc};
    use tracing_subscriber::{
        filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter,
    };

    static INIT: Once = Once::new();

    fn init() {
        INIT.call_once(|| {
            let _ = tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer())
                .with(
                    EnvFilter::builder()
                        .with_default_directive(LevelFilter::TRACE.into())
                        .from_env_lossy(),
                )
                .try_init();
        });
    }

    fn generate_random_string(prefix: &str, length: usize) -> String {
        let mut rng = rand::rng();
        let random_string: String = (0..length)
            .map(|_| rng.sample(Alphanumeric) as char)
            .collect();
        format!("{}_{}", prefix, random_string)
    }

    #[cfg(feature = "protocal_gp")]
    #[test]
    fn test_binary() {
        use crate::FifoHandlerInfo;

        init();
        let path = "../cupti_fifo_1098952-1753759453406908844.raw";
        let (_, ts_off) = path.split_once('-').unwrap();
        let (ts_off, _) = ts_off.split_once('.').unwrap();
        let ts_off = ts_off.parse::<usize>().unwrap();
        log::info!("ts_off: {}", ts_off);

        let mut fd = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(path)
            .unwrap();

        let mut rng = rand::rng();
        let id = rng.random::<u32>() as usize;
        let name = generate_random_string("fifo_cupti", 8);
        let receiver_name = name.clone();
        let mut count = 0;
        let base = "../testcases";

        let output = Path::new(base).join("output");

        let (info_tx, mut info_rx) = broadcast::channel(64);

        let (upstream_tx, mut upstream_rx) = mpsc::unbounded_channel::<UpstreamMessage>();
        std::thread::spawn(move || {
            receiver::upstream_receiver(
                output,
                &mut upstream_rx,
                info_tx,
                receiver_name,
                ts_off,
                0,
                false,
                None,
            )
        });

        upstream_tx
            .send(UpstreamMessage::Up((id, name.to_string())))
            .unwrap();

        loop {
            let mut buf = [0; TILE_SIZE + 64];
            match fd.read(&mut buf) {
                Ok(0) => {
                    upstream_tx
                        .send(UpstreamMessage::Finish((id, name.to_string())))
                        .unwrap();
                    break;
                }
                Ok(n) => {
                    count += 1;
                    let (header, content) = buf.split_at_mut(64);
                    let (size_part, _) = header.split_at_mut(31);
                    let (_, size) = size_part.split_at_mut(15);
                    let size = String::from_utf8(size.into()).unwrap();
                    let size = usize::from_str_radix(&size, 16).unwrap();
                    log::debug!(
                        "content len {:?}\nheader:\n{:?}\n{size}",
                        n,
                        header.hex_dump()
                    );
                    upstream_tx
                        .send(UpstreamMessage::Buffer((
                            id,
                            name.to_string(),
                            content.try_into().unwrap(),
                            size,
                        )))
                        .unwrap();
                }
                Err(e) => {
                    log::error!("read error: {}", e);
                }
            }
        }
        log::info!("test count: {}", count);

        loop {
            match info_rx.try_recv() {
                Ok(FifoHandlerInfo::Finish(name)) => {
                    log::info!("{} finish", name);
                    break;
                }
                Ok(info) => {
                    log::info!("info: {:?}", info);
                }
                Err(_) => {}
            }
        }
    }
}
