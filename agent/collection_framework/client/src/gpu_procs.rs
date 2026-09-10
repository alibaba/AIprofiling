// Query the host's active CUDA compute processes by running
// `nvidia-smi --query-compute-apps=pid,process_name,used_gpu_memory
// --format=csv,noheader,nounits`. Returns an empty Vec when nvidia-smi
// prints nothing (no GPU jobs), and Err when the binary is missing
// or exits non-zero.

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;

use crate::protocol::GpuProc;

pub async fn list_gpu_procs() -> Result<Vec<GpuProc>> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,process_name,used_gpu_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
        .context("spawn nvidia-smi")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(anyhow!(
            "nvidia-smi exited {}: {}",
            out.status,
            if stderr.is_empty() { "<no stderr>".into() } else { stderr }
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(parse_csv(&stdout))
}

fn parse_csv(text: &str) -> Vec<GpuProc> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let mut parts = line.split(',').map(str::trim);
            let pid = parts.next()?.parse::<u32>().ok()?;
            let name = parts.next().unwrap_or("").to_string();
            let mem = parts
                .next()
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            Some(GpuProc { pid, name, mem_mib: mem })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_output() {
        let raw = "\
2815111, python3, 512\n\
2815200, torch_worker, 1024\n";
        let v = parse_csv(raw);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0], GpuProc { pid: 2815111, name: "python3".into(), mem_mib: 512 });
        assert_eq!(v[1].pid, 2815200);
    }

    #[test]
    fn empty_when_no_procs() {
        assert!(parse_csv("").is_empty());
        assert!(parse_csv("\n\n").is_empty());
    }

    #[test]
    fn skips_bad_rows_but_keeps_good_ones() {
        let raw = "not-a-pid, x, 0\n999, py, 128\n";
        let v = parse_csv(raw);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].pid, 999);
    }
}
