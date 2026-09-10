use nix::{errno, sys::stat, unistd::mkfifo as _mkfifo};
use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs::{FileTypeExt as _, PermissionsExt},
    path::Path,
};
use tokio::sync::{broadcast, mpsc};

use tracing as log;
#[cfg(feature = "protocal_gp")]
mod bin_parser;
mod controller;
#[cfg(feature = "protocal_ascii")]
mod cupti_parser;
pub mod receiver;
mod utils;
mod worker;

type GenericErr = Box<dyn std::error::Error + Send + Sync>;
type GenericResult = Result<(), GenericErr>;

#[cfg(feature = "protocal_gp")]
pub const TILE_SIZE: usize = 16 * 1024;
#[cfg(feature = "protocal_ascii")]
const TILE_SIZE: usize = 32 * 1024;

#[cfg(feature = "protocal_gp")]
const HEADER_SIZE: usize = 8;
#[cfg(feature = "protocal_gp")]
const HEADER_SECTION_SIZE: usize = 4;

const READ_BUFFER: usize = 1024 * 1024 * 2;

// FIFO max buffer size set to 32MB
const FIFO_BUFFER: i32 = 1024 * 1024 * 32;

#[derive(Debug, Clone)]
pub enum UpstreamMessage {
    Up((usize, String)),
    _Payload((usize, String, String)),
    Buffer((usize, String, [u8; TILE_SIZE], usize)),
    Finish((usize, String)),
}

/// Checks if the file is a FIFO.
fn is_fifo<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    Ok(std::fs::metadata(path)?.file_type().is_fifo())
}

fn check_output_directory(output: &Path) -> GenericResult {
    if output.exists() {
        if output.is_dir() {
            Ok(())
        } else {
            Err(format!(
                "output path {} is not a directory",
                output.to_string_lossy()
            )
            .into())
        }
    } else {
        std::fs::create_dir_all(output)?;
        Ok(())
    }
}

pub fn app<P: AsRef<Path>>(
    info: broadcast::Sender<FifoHandlerInfo>,
    cmd: mpsc::UnboundedReceiver<FifoHandlerCommand>,
    output: P,
) {
    let output = output.as_ref().to_path_buf();

    check_output_directory(output.as_path())
        .expect("'output' should be a valid and accessible directory.");
    controller::controller(output, info, cmd).expect("controller failed");
}

#[derive(Debug, Clone, PartialEq)]
pub enum FifoHandlerInfo {
    Ready(String),
    Deleted(String),
    Finish(String),
}

#[derive(Debug, Clone)]
pub enum FifoHandlerCommand {
    Listen(
        (
            usize,
            String,
            String,
            usize,
            bool,
            Option<HashMap<u32, u32>>,
        ),
    ),
}

pub struct FifoHandler {
    info: broadcast::Receiver<FifoHandlerInfo>,
    cmd: mpsc::UnboundedSender<FifoHandlerCommand>,
}

impl FifoHandler {
    pub fn new(
        info: broadcast::Receiver<FifoHandlerInfo>,
        cmd: mpsc::UnboundedSender<FifoHandlerCommand>,
    ) -> Self {
        Self { info, cmd }
    }

    pub fn setup(
        &self,
        id: usize,
        name: &str,
        path: &str,
        limit: usize,
        legacy_output: bool,
        pid_maps: Option<HashMap<u32, u32>>,
    ) -> Result<FifoKnob, GenericErr> {
        self.cmd.send(FifoHandlerCommand::Listen((
            id,
            name.to_string(),
            path.to_string(),
            limit,
            legacy_output,
            pid_maps,
        )))?;
        Ok(FifoKnob::new(self.info.resubscribe(), name, id))
    }
}

impl Drop for FifoHandler {
    fn drop(&mut self) {
        while let Ok(msg) = self.info.try_recv() {
            log::trace!("remaining info: {:?}", msg);
        }
    }
}

pub struct FifoKnob {
    info: broadcast::Receiver<FifoHandlerInfo>,
    pub name: String,
    pub id: usize,
}

impl FifoKnob {
    pub fn new(info: broadcast::Receiver<FifoHandlerInfo>, name: &str, id: usize) -> Self {
        Self {
            info,
            name: name.to_string(),
            id: id,
        }
    }

    async fn poll_info_till(&mut self, state: FifoHandlerInfo) {
        loop {
            match self.info.recv().await {
                Ok(msg) => {
                    if msg == state {
                        break;
                    }
                }
                Err(err) => {
                    panic!("fifo handler info channel closed, err: {}", err);
                }
            }
        }
    }

    /// Wait for finish, if not finish, poll info untill finish.
    /// So this function will block, and block forever if not finish.
    /// If you want to wait for finish with timeout, use wait_finish_timeout.
    pub async fn wait_finish(&mut self) {
        let name = self.name.clone();
        self.poll_info_till(FifoHandlerInfo::Finish(name.clone()))
            .await;
        log::info!("FIFO finish: {} done", name);
    }

    /// Return error if timeout, otherwise return Ok.
    /// Timeout is in milliseconds
    pub async fn wait_finish_timeout(&mut self, timeout: u64) -> GenericResult {
        let name = self.name.clone();
        tokio::select! {
            _ = self.poll_info_till(FifoHandlerInfo::Finish(name.clone())) => {
                Ok(())
            },
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(timeout)) => {
                Err("wait finish timeout".into())
            }
        }
    }

    pub fn wait_finish_timeout_sync(&mut self, timeout: u64) -> GenericResult {
        let name = self.name.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?
            .block_on(async {
                tokio::time::timeout(
                    tokio::time::Duration::from_millis(timeout),
                    self.poll_info_till(FifoHandlerInfo::Finish(name.clone())),
                )
                .await
            })?;
        Ok(())
    }

    /// Return error if timeout, otherwise return Ok.
    /// Timeout is in milliseconds
    pub async fn wait_ready_timeout(&mut self, timeout: u64) -> GenericResult {
        let name = self.name.clone();
        tokio::select! {
            _ = self.poll_info_till(FifoHandlerInfo::Ready(name.clone())) => {
                Ok(())
            },
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(timeout)) => {
                Err("wait finish timeout".into())
            }
        }
    }

    pub fn wait_ready_timeout_sync(&mut self, timeout: u64) -> GenericResult {
        let name = self.name.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()?
            .block_on(async {
                tokio::time::timeout(
                    tokio::time::Duration::from_millis(timeout),
                    self.poll_info_till(FifoHandlerInfo::Ready(name.clone())),
                )
                .await
            })?;
        Ok(())
    }
}

fn mkfifo<P: AsRef<Path>>(path: P) -> GenericResult {
    let p = path.as_ref();
    match _mkfifo(p, stat::Mode::S_IRWXU) {
        Ok(_) => {
            fs::set_permissions(path, fs::Permissions::from_mode(0o622))?;
            Ok(())
        }
        Err(errno::Errno::EEXIST) => {
            if is_fifo(&path)? {
                log::debug!("fifo file already exists");
                Ok(())
            } else {
                Err(GenericErr::from("invalid fifo file name"))
            }
        }
        Err(err) => Err(GenericErr::from(err)),
    }
}

fn rmfifo<P: AsRef<Path>>(path: P, ns: Option<usize>) -> GenericResult {
    match std::fs::remove_file(&path) {
        Ok(_) => {
            log::debug!("rmfifo {:?} success", path.as_ref());
            Ok(())
        }
        Err(err) => {
            log::debug!("try slow_path {:?} error: {}", path.as_ref(), err);
            if let Some(next) = find_ns_shared_pids(ns)
                .ok()
                .and_then(|mut pids| pids.next())
            {
                let next_path = Path::new("/proc").join(next.to_string()).join("root/tmp");
                rmfifo(&next_path, None)
            } else {
                Err(GenericErr::from(err))
            }
        }
    }
}

fn get_pid_ns(pid: usize) -> Result<usize, GenericErr> {
    let ns_path = std::fs::read_link(format!("/proc/{}/ns/pid", pid))?.into_os_string();
    let ns_link = ns_path.to_str().ok_or("invalid ns link")?;

    // ns_link format just exactly like: "pid:[4026539420]"
    Ok(ns_link
        .strip_prefix("pid:[")
        .ok_or("invalid ns link prefix")?
        .strip_suffix("]")
        .ok_or("invalid ns link suffix")?
        .parse::<usize>()?)
}

fn find_ns_shared_pids(ns: Option<usize>) -> Result<impl Iterator<Item = usize>, GenericErr> {
    let ns = ns.ok_or("invalid ns")?;
    Ok(std::fs::read_dir("/proc")?
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok()?.parse::<usize>().ok())
        .filter_map(move |pid| {
            if get_pid_ns(pid).ok()? == ns {
                Some(pid)
            } else {
                None
            }
        }))
}

#[macro_export]
macro_rules! fifo_server_run {
    ($output:expr, $brocast_num:expr) => {{
        let (info_tx, info_rx) = tokio::sync::broadcast::channel($brocast_num);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            std::sync::Arc::new($crate::FifoHandler::new(info_rx, cmd_tx)),
            std::thread::spawn(move || $crate::app(info_tx, cmd_rx, $output)),
        )
    }};
    ($output:expr) => {{
        let (info_tx, info_rx) = tokio::sync::broadcast::channel(64);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        (
            std::sync::Arc::new($crate::FifoHandler::new(info_rx, cmd_tx)),
            std::thread::spawn(move || $crate::app(info_tx, cmd_rx, $output)),
        )
    }};
}

#[macro_export]
macro_rules! concat_strings {
    (; $append_postfix:expr) => {{
        $append_postfix.to_string()
    }};
    ($($s:expr),+ ; $append_postfix:expr) => {{
        let mut result = vec![$($s.to_string()),+].join(",");
        result.push_str(&$append_postfix.to_string());
        result
    }};
    ($($s:expr),*) => {{
        vec![$($s.to_string()),*].join(",")
    }};
}

#[macro_export]
macro_rules! include_header {
    ($package: tt) => {
        include!(concat!(env!("OUT_DIR"), concat!("/", $package, ".rs")));
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::join_all;
    use rand::{distr::Alphanumeric, Rng};
    use std::process::Command;
    use std::sync::Once;
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
                        .with_default_directive(LevelFilter::INFO.into())
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

    #[test]
    fn test_mkfifo() {
        init();
        let path = std::path::Path::new("../testcases/test.fifo");
        mkfifo(path).unwrap();
        mkfifo(path).unwrap();
        rmfifo(path, None).unwrap();

        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)
            .unwrap();

        let result = mkfifo(path);
        assert!(result.is_err());
        rmfifo(path, None).unwrap();
    }

    #[test]
    fn test_find_namespace() {
        init();
        let pid: usize = std::process::id() as usize;
        let ns = get_pid_ns(pid).unwrap();
        log::info!("pid_ns: {:?}", ns);
        let pids = find_ns_shared_pids(Some(ns))
            .unwrap()
            .collect::<Vec<usize>>();
        log::info!("shared_pids: {:?}", pids);
    }

    #[test]
    fn test_concat_strings() {
        init();
        let s1 = "a".to_string();
        let s2 = "b".to_string();

        // postfix with comma
        assert_eq!(concat_strings!(s1, s2; ","), "a,b,");

        // postfix none
        assert_eq!(concat_strings!(s1, s2), "a,b");

        // zero input, postfix with comma only
        assert_eq!(concat_strings!(; ","), ",");

        // postfix with anything
        assert_eq!(concat_strings!(s1, s2; "][]cd"), "a,b][]cd");
    }

    #[tokio::test]
    async fn test_cupti() {
        init();
        // maximum number of fifo now can be 60
        run_test(1).await
    }

    async fn run_test(num: usize) {
        let mut fifo_interests = Vec::new();
        // let mut rng = rand::rng();
        for _ in 0..num {
            fifo_interests.push((
                1000, //rng.random::<u32>() as usize,
                generate_random_string("fifo_cupti", 8),
            ));
        }

        let base = "../testcases";
        let output = Path::new(base).join(base).join("output");
        let output_clone = output.clone();
        let source = "fifo_cupti.c";
        #[cfg(feature = "protocal_ascii")]
        let dataset = "dataset.tar.xz";
        #[cfg(feature = "protocal_ascii")]
        let data = "dataset.json";
        #[cfg(feature = "protocal_gp")]
        let dataset = "bin-dataset.tar.xz";
        #[cfg(feature = "protocal_gp")]
        let data = "bin-data.json";

        let notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let mut tasks = Vec::new();
        let (handle, _) = fifo_server_run!(output_clone);

        let knobs = fifo_interests
            .clone()
            .iter()
            .map(|(id, name)| {
                handle
                    .setup(*id, name, &format!("{base}/fifo"), 0, false, None)
                    .unwrap()
            })
            .collect::<Vec<FifoKnob>>();

        for mut knob in knobs {
            knob.wait_ready_timeout(500).await.unwrap();
            let fifo_interest = knob.name.clone();
            let notify1 = notify.clone();
            let notify2 = notify.clone();

            let base = base.to_string();

            let task = tokio::spawn(async move {
                let target = format!("{base}/a.out");
                Command::new("gcc")
                    .arg("-o")
                    .arg(&target)
                    .arg(format!("{base}/{source}"))
                    .output()
                    .unwrap();
                let data_path = Path::new(&base).join(data);
                if !data_path.exists() {
                    Command::new("tar")
                        .arg("-xJf")
                        .arg(format!("{base}/{dataset}"))
                        .output()
                        .unwrap();
                }
                notify1.notified().await;
                let span = tracing::info_span!("copy").entered();
                log::info!("start");
                Command::new(&target)
                    .arg(format!("{base}/{data}"))
                    .arg(format!("{base}/fifo/{fifo_interest}"))
                    .spawn()
                    .unwrap();
                log::info!("finished");
                span.exit();
            });
            tasks.push(task);
            let task = tokio::spawn(async move {
                notify2.notify_one();
                knob.wait_finish().await;
            });
            tasks.push(task);
        }

        join_all(tasks).await;

        log::info!("fifo read all finished");
        let md5_set = fifo_interests
            .iter()
            .filter_map(|(_, fifo_interest)| {
                std::fs::read_dir(output.clone())
                    .ok()?
                    .filter_map(|entry| entry.ok())
                    .find(|entry| {
                        if let Some(name) = entry.file_name().to_str() {
                            name.contains(&format!("{}-", fifo_interest))
                        } else {
                            false
                        }
                    })
            })
            .map(|entry| {
                String::from_utf8(
                    Command::new("md5sum")
                        .arg(entry.path())
                        .output()
                        .unwrap()
                        .stdout,
                )
                .unwrap()
            })
            .filter_map(|s| Some(s.split_whitespace().next()?.to_string()))
            .fold(std::collections::HashSet::new(), |mut acc, x| {
                acc.insert(x);
                acc
            });
        log::info!("md5_set: {:?}", md5_set);
        // every time running, reading fifo should get the same md5, so md5 set len should be 1
        assert_eq!(md5_set.len(), 1);
    }
}
