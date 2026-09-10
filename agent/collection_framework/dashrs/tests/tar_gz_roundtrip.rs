// Integration test: exercise `tar_gz_dir` on a realistic directory tree
// and verify the archive is a valid gzip'd USTAR containing every input file.

use std::collections::HashMap;
use std::fs;
use std::io::Read;

use dashrs::tar_gz_dir;
use flate2::read::GzDecoder;
use tar::Archive;

#[test]
fn packages_a_realistic_output_directory() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path();

    // Simulate the CF `output/` layout we ship to the dashboard.
    fs::create_dir_all(src.join("AIProf_1234")).unwrap();
    fs::write(
        src.join("AIProf_1234/summary.json"),
        br#"{"pid":1234,"duration":5}"#,
    )
    .unwrap();
    fs::write(
        src.join("AIProf_1234/cuda_trace.log"),
        b"CUDA event log line 1\nCUDA event log line 2\n",
    )
    .unwrap();
    fs::create_dir(src.join("AIProf_1234/gpu")).unwrap();
    let big: Vec<u8> = (0u8..=255).cycle().take(64 * 1024).collect();
    fs::write(src.join("AIProf_1234/gpu/binary.bin"), &big).unwrap();

    let archive = tar_gz_dir(src).expect("packaging must succeed");
    assert!(archive.len() > 64, "archive is suspiciously small: {}", archive.len());

    // Re-open and reconstruct the file map.
    let mut seen: HashMap<String, Vec<u8>> = HashMap::new();
    let gz = GzDecoder::new(&archive[..]);
    let mut ar = Archive::new(gz);
    for entry in ar.entries().unwrap() {
        let mut e = entry.unwrap();
        if e.header().entry_type().is_dir() {
            continue;
        }
        let path = e.path().unwrap().to_string_lossy().to_string();
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).unwrap();
        seen.insert(path, buf);
    }

    let contains = |suffix: &str| -> Vec<u8> {
        seen.iter()
            .find(|(k, _)| k.ends_with(suffix))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| panic!("missing {} in {:?}", suffix, seen.keys().collect::<Vec<_>>()))
    };

    assert_eq!(
        contains("AIProf_1234/summary.json"),
        br#"{"pid":1234,"duration":5}"#
    );
    assert_eq!(
        contains("AIProf_1234/cuda_trace.log"),
        b"CUDA event log line 1\nCUDA event log line 2\n"
    );
    assert_eq!(contains("AIProf_1234/gpu/binary.bin"), big);
}
