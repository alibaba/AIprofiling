// Pack a directory tree into a gzip'd tar archive, in memory.
//
// The AIProf dashboard collector caps a single multipart upload at 3 GB
// (`dashboardServer.js` multer config). Anything within that budget can be
// staged in a `Vec<u8>` cheaply; for genuinely larger jobs, callers should
// tar to a temp file and stream via `reqwest::Body::wrap_stream`. That
// streaming path is a future addition — see the crate-level docs.

use std::path::Path;

use flate2::{write::GzEncoder, Compression};
use tar::Builder;

use crate::error::{DashError, Result};

/// Recursively tar+gz the contents of `dir`, returning the archive bytes.
///
/// Entry paths inside the archive are relative to `dir` (rooted at `.`),
/// so absolute paths from the caller's filesystem are not leaked.
pub fn tar_gz_dir(dir: &Path) -> Result<Vec<u8>> {
    if !dir.exists() {
        return Err(DashError::PackageError(format!(
            "source directory {:?} does not exist",
            dir
        )));
    }
    if !dir.is_dir() {
        return Err(DashError::PackageError(format!(
            "source path {:?} is not a directory",
            dir
        )));
    }

    let mut buf: Vec<u8> = Vec::new();
    {
        let gz = GzEncoder::new(&mut buf, Compression::default());
        let mut tar = Builder::new(gz);
        tar.follow_symlinks(false);
        tar.append_dir_all(".", dir)
            .map_err(|e| DashError::PackageError(format!("append_dir_all({:?}): {}", dir, e)))?;
        let gz = tar
            .into_inner()
            .map_err(|e| DashError::PackageError(format!("finalize tar: {}", e)))?;
        gz.finish()
            .map_err(|e| DashError::PackageError(format!("finish gzip: {}", e)))?;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::collections::BTreeMap;
    use std::fs;
    use std::io::Read;
    use tar::Archive;

    #[test]
    fn rejects_missing_dir() {
        let err = tar_gz_dir(Path::new("/nonexistent/dashrs/xyzzy")).unwrap_err();
        assert!(matches!(err, DashError::PackageError(_)));
    }

    #[test]
    fn rejects_file_input() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("plain.txt");
        fs::write(&f, b"hello").unwrap();
        let err = tar_gz_dir(&f).unwrap_err();
        assert!(matches!(err, DashError::PackageError(_)));
    }

    #[test]
    fn round_trip_preserves_content() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path();
        fs::write(src.join("a.txt"), b"alpha").unwrap();
        fs::write(src.join("empty.bin"), b"").unwrap();
        fs::create_dir(src.join("sub")).unwrap();
        // Include some binary noise to catch encoding bugs.
        let bin: Vec<u8> = (0u8..=255u8).cycle().take(4096).collect();
        fs::write(src.join("sub/binary.dat"), &bin).unwrap();
        fs::write(src.join("sub/nested.txt"), b"deep").unwrap();

        let bytes = tar_gz_dir(src).unwrap();
        assert!(!bytes.is_empty());

        let mut extracted: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let gz = GzDecoder::new(&bytes[..]);
        let mut ar = Archive::new(gz);
        for entry in ar.entries().unwrap() {
            let mut e = entry.unwrap();
            let path = e.path().unwrap().to_string_lossy().to_string();
            if e.header().entry_type().is_dir() {
                continue;
            }
            let mut data = Vec::new();
            e.read_to_end(&mut data).unwrap();
            extracted.insert(path, data);
        }
        // Paths are prefixed with "./" from append_dir_all(".", ..).
        let find = |name: &str| {
            extracted
                .iter()
                .find(|(k, _)| k.ends_with(name))
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("missing {} in archive: {:?}", name, extracted.keys().collect::<Vec<_>>()))
        };
        assert_eq!(find("a.txt"), b"alpha");
        assert_eq!(find("empty.bin"), b"");
        assert_eq!(find("sub/binary.dat"), bin);
        assert_eq!(find("sub/nested.txt"), b"deep");
    }
}
