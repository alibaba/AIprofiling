// Compress `.pickle` files in the workdir (matching the legacy
// `find … | xargs gzip` shell pipeline) and hand the whole directory
// to dashrs for tar.gz + multipart upload to the dashboard collector.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;
use tracing as log;
use walkdir::WalkDir;

use crate::config::Config;
use dashrs::{ClientIdentity, DashClient, DashConfig};

#[derive(Clone)]
pub struct Uploader {
    client: DashClient,
}

impl Uploader {
    pub fn new(cfg: &Config) -> Result<Self> {
        let identity = ClientIdentity::resolve(
            Some(&cfg.client_id),
            Some(&cfg.namespace),
            Some(&cfg.pod_name),
            Some(&cfg.node_name),
        )
        .context("resolve dashrs ClientIdentity")?;
        let dash_cfg = DashConfig::new_defaults(cfg.http_url.clone(), identity);
        let client = DashClient::new(dash_cfg).context("build dashrs DashClient")?;
        Ok(Self { client })
    }

    /// Compress pickles in place, then tar.gz the whole `workdir` and
    /// upload as `task_id`. Errors from any step propagate — the caller
    /// (TaskRunner) turns them into a TASK_RESULT failure.
    pub async fn upload(&self, workdir: &Path, task_id: &str) -> Result<()> {
        compress_pickles(workdir).await?;
        let response = self
            .client
            .upload_dir(workdir, task_id)
            .await
            .with_context(|| format!("dashrs upload task_id={}", task_id))?;
        log::info!(
            task_id = %response.task_id,
            code = %response.code,
            "upload complete"
        );
        Ok(())
    }
}

/// Replace every `foo.pickle` file under `dir` with `foo.pickle.gz`.
/// Pure Rust — no shell, no `find`, no `xargs`.
pub async fn compress_pickles(dir: &Path) -> Result<()> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || compress_pickles_sync(&dir))
        .await
        .context("compress_pickles join")??;
    Ok(())
}

fn compress_pickles_sync(dir: &Path) -> Result<()> {
    for entry in WalkDir::new(dir).into_iter().filter_map(|r| r.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|s| s.to_str()) != Some("pickle") {
            continue;
        }
        let src = entry.path();
        let dst = src.with_extension("pickle.gz");
        let data = std::fs::read(src)
            .with_context(|| format!("read pickle {:?}", src))?;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&data)
            .with_context(|| format!("gzip-write pickle {:?}", src))?;
        let gzipped = enc.finish()
            .with_context(|| format!("gzip-finish pickle {:?}", src))?;
        std::fs::write(&dst, gzipped)
            .with_context(|| format!("write gz {:?}", dst))?;
        std::fs::remove_file(src)
            .with_context(|| format!("remove original pickle {:?}", src))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn compress_pickles_replaces_files() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a.pickle"), b"hello").unwrap();
        std::fs::write(td.path().join("b.pickle"), b"world").unwrap();
        std::fs::write(td.path().join("c.txt"), b"skip me").unwrap();
        let sub = td.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("d.pickle"), b"nested").unwrap();

        compress_pickles(td.path()).await.unwrap();

        assert!(!td.path().join("a.pickle").exists());
        assert!(!td.path().join("b.pickle").exists());
        assert!(!sub.join("d.pickle").exists());
        assert!(td.path().join("a.pickle.gz").exists());
        assert!(td.path().join("b.pickle.gz").exists());
        assert!(sub.join("d.pickle.gz").exists());
        assert!(td.path().join("c.txt").exists());

        let bytes = std::fs::read(td.path().join("a.pickle.gz")).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&bytes[..]);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut out).unwrap();
        assert_eq!(out, b"hello");
    }

    #[tokio::test]
    async fn compress_pickles_on_empty_dir_is_noop() {
        let td = tempfile::tempdir().unwrap();
        compress_pickles(td.path()).await.unwrap();
    }
}
