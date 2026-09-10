// Build and run the `CollectionFramework profile` subprocess for one
// NEW_TASK, translating the wire-level ProfilingConfig into CLI flags.
//
// Reentrancy: CF's in-process scheduler cannot be reused (OnceLock
// singletons in event_handler, timer leaks, dlopen'd plugin cleanup),
// so every task must spawn a fresh subprocess. The kill_on_drop flag
// ensures a lost tokio::spawn drops the subprocess too.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use tokio::process::Command;
use tracing as log;

use crate::protocol::ProfilingConfig;

const CF_BINARY: &str = "./CollectionFramework";

/// Assemble the CLI. Extracted so tests can inspect the arg list.
pub fn build_command(cf_binary: &str, workdir: &Path, cfg: &ProfilingConfig) -> Command {
    let mut cmd = Command::new(cf_binary);
    // CF's --verbose defaults to 1 (INFO); no need to pass it explicitly.
    cmd.arg("profile")
        .arg("--merge")
        .arg("true")
        .arg("--output")
        .arg(workdir);

    if let Some(timeout_ms) = cfg.timeout.filter(|&t| t > 0) {
        let secs = timeout_ms / 1000;
        cmd.arg("--duration").arg(secs.to_string());
    } else if let Some((start, end)) = cfg.iteration_range() {
        cmd.arg("--iteration")
            .arg("--num-steps")
            .arg(end.saturating_sub(start).to_string())
            .arg("--num-skip-steps")
            .arg(start.to_string());
        if let Some(m) = cfg.effective_iteration_module() {
            cmd.arg("--iteration-module").arg(m);
        }
        if let Some(f) = cfg.effective_iteration_function() {
            cmd.arg("--iteration-function").arg(f);
        }
    }

    apply_analysis_params(&mut cmd, &cfg.analysis_params);

    let pids = cfg.pids.to_vec();
    if !pids.is_empty() {
        cmd.arg("--pids");
        for pid in pids {
            cmd.arg(pid);
        }
    }
    cmd
}

fn apply_analysis_params(cmd: &mut Command, params: &[String]) {
    for p in params {
        match p.as_str() {
            "adapt" => { cmd.arg("--adapt").arg("true"); }
            "snapshot" => {
                cmd.arg("--snapshot").arg("true");
                cmd.arg("--profile-memory").arg("true");
            }
            "python" => { cmd.arg("--pystack").arg("true"); }
            "kernel" => { cmd.arg("--gpu").arg("true"); }
            "pytorch" => { cmd.arg("--torch").arg("true"); }
            other => {
                log::warn!(param = %other, "unknown analysis_param, ignoring");
            }
        }
    }
}

/// Per-task workdir. Isolates outputs from parallel invocations (though
/// TaskRunner serializes at the moment) and gives us a deterministic
/// path to tar+upload.
pub fn workdir_for(analysis_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("aiprof-{}", analysis_id))
}

/// Grace period added on top of the profiling duration before the client
/// force-kills a wedged CollectionFramework. CF has its own scheduler
/// watchdog; this is the outer backstop for the case where the whole
/// subprocess hangs (e.g. a plugin deadlocks before the scheduler loop even
/// starts). Generous so a slow-but-live merge/upload is never cut short.
const RUN_GRACE_SECS: u64 = 120;

/// Wall-clock budget for one CF subprocess. Duration mode gets
/// `duration + grace`; iteration mode has no fixed duration, so we fall back
/// to a generous hard cap that still guarantees the client self-heals.
fn run_budget(cfg: &ProfilingConfig) -> std::time::Duration {
    match cfg.timeout.filter(|&t| t > 0) {
        Some(ms) => std::time::Duration::from_secs(ms / 1000 + RUN_GRACE_SECS),
        None => std::time::Duration::from_secs(900),
    }
}

/// Run the profiler subprocess to completion. `workdir` must already exist.
///
/// A hard timeout wraps the subprocess: if CF wedges (see the CF-side
/// scheduler watchdog and the merge-meta deadlock), the client SIGKILLs it
/// and returns an error instead of holding the TaskRunner mutex forever —
/// which would make the server refuse every subsequent task with
/// "is Profiling, refuse new task".
pub async fn run(
    workdir: &Path,
    cfg: &ProfilingConfig,
) -> Result<()> {
    let mut cmd = build_command(CF_BINARY, workdir, cfg);
    cmd.kill_on_drop(true);
    // Put CF in its own process group so a timeout can reap the whole tree
    // (CF spawns a `pyki-inject` grandchild that ptrace-attaches the target;
    // killing only CF would orphan it, leaving the target frozen).
    cmd.process_group(0);
    let budget = run_budget(cfg);
    log::debug!(?cmd, ?budget, "spawning CollectionFramework");
    let mut child = cmd.spawn()?;
    let pgid = child.id().map(|p| p as libc::pid_t);
    match tokio::time::timeout(budget, child.wait()).await {
        Ok(Ok(status)) => {
            if !status.success() {
                return Err(anyhow!("CollectionFramework profile exited with {}", status));
            }
            Ok(())
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_elapsed) => {
            log::error!(?budget, "CollectionFramework exceeded budget, killing process group");
            // Signal the whole group; CF's pid doubles as the group id.
            if let Some(pgid) = pgid {
                unsafe { libc::kill(-pgid, libc::SIGKILL); }
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
            Err(anyhow!(
                "CollectionFramework timed out after {:?} and was killed",
                budget
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args_of(cmd: &Command) -> Vec<String> {
        cmd.as_std()
            .get_args()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn duration_mode_builds_expected_args() {
        let cfg = ProfilingConfig {
            timeout: Some(3000),
            pids: crate::protocol::PidList::Csv("1234".into()),
            analysis_params: vec!["adapt".into()],
            ..Default::default()
        };
        let cmd = build_command("./CF", &PathBuf::from("/tmp/w"), &cfg);
        let args = args_of(&cmd);
        assert!(args.contains(&"profile".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "--duration" && w[1] == "3"));
        assert!(args.contains(&"--adapt".to_string()));
        assert!(args.contains(&"--pids".to_string()));
        assert!(args.contains(&"1234".to_string()));
        assert!(!args.contains(&"--iteration".to_string()));
    }

    #[test]
    fn iteration_mode_builds_iteration_flags() {
        let cfg = ProfilingConfig {
            iteration: Some(vec![2, 7]),
            iteration_mod: Some("mod_a".into()),
            iteration_func: Some("fn_a".into()),
            ..Default::default()
        };
        let cmd = build_command("./CF", &PathBuf::from("/tmp/w"), &cfg);
        let args = args_of(&cmd);
        assert!(args.contains(&"--iteration".to_string()));
        assert!(args.windows(2).any(|w| w[0] == "--num-steps" && w[1] == "5"));
        assert!(args.windows(2).any(|w| w[0] == "--num-skip-steps" && w[1] == "2"));
        assert!(args.windows(2).any(|w| w[0] == "--iteration-module" && w[1] == "mod_a"));
        assert!(args.windows(2).any(|w| w[0] == "--iteration-function" && w[1] == "fn_a"));
        assert!(!args.contains(&"--duration".to_string()));
    }

    #[test]
    fn unknown_analysis_params_are_dropped() {
        let cfg = ProfilingConfig {
            timeout: Some(1000),
            analysis_params: vec![
                "nope".into(),
                "adapt".into(),
                "nvtx".into(),
                "dcgm".into(),
                "rdma".into(),
                "net".into(),
                "memory".into(),
                "shapes".into(),
                "flops".into(),
            ],
            ..Default::default()
        };
        let cmd = build_command("./CF", &PathBuf::from("/tmp/w"), &cfg);
        let args = args_of(&cmd);
        assert!(args.contains(&"--adapt".to_string()));
        assert!(!args.iter().any(|a| a == "--nope"));
        assert!(!args.iter().any(|a| a == "--nvtx"));
        assert!(!args.iter().any(|a| a == "--dcgm"));
        assert!(!args.iter().any(|a| a == "--rdma"));
        assert!(!args.iter().any(|a| a == "--net"));
        assert!(!args.iter().any(|a| a == "--profile-memory"));
        assert!(!args.iter().any(|a| a == "--record-shapes"));
        assert!(!args.iter().any(|a| a == "--flops"));
    }

    #[test]
    fn workdir_uses_analysis_id() {
        let p = workdir_for("abcd-1234");
        assert!(p.to_string_lossy().contains("aiprof-abcd-1234"));
    }
}
