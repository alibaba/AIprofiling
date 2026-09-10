use crate::tools::gpu_process::list_gpu_processes;
use crate::r#const;
use crate::{collector::indicator_name::IndicatorName, error::ErrorCode};
use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::env;
use std::path::Path;
use tracing as log;

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (commit: ",
    env!("GIT_COMMIT_ID"),
    ")"
);

#[derive(Parser, Debug)]
#[command(author, version = VERSION, about, long_about = None)]
pub struct Args {
    /// Log verbosity level
    #[arg(long, value_name = "verbose", default_value = "1")]
    pub verbose: u8,

    /// Log directory; when unset, logs go only to the terminal
    #[arg(long, value_name = "LogDir")]
    pub log_dir: Option<String>,

    /// Subcommand
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Parser, Debug, Clone)]
pub enum Commands {
    /// Run the main control flow (collection, analysis, etc.)
    Profile(ProfileArgs),

    /// Inject a probe into the target process
    PykiInject(InjectArgs),

    /// Inject the CUPTI library into the target process
    CuptiInject(CuptiInjectArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct InjectArgs {
    /// Target process ID (injection target)
    #[arg(long, value_name = "TargetPid", required = true)]
    pub target_pid: i32,

    /// Output path. output=Some("") writes to the source directory; output=None writes to cwd
    #[arg(long, value_name = "Output", num_args(0..=1))]
    pub output: Option<String>,

    #[arg(long, value_name = "PykiArgs")]
    pub pyki_args: String,
}

#[derive(Parser, Debug, Clone)]
pub struct CuptiInjectArgs {
    /// Target process ID (injection target)
    #[arg(long, value_name = "TargetPid", required = true)]
    pub target_pid: i32,

    /// Output path. output=Some("") writes to the source directory; output=None writes to cwd
    #[arg(long, value_name = "Output", num_args(0..=1))]
    pub output: Option<String>,
}

#[derive(Parser, Debug, serde::Serialize, serde::Deserialize, Clone)]
#[command(author, version, about, long_about = None)]
pub struct ProfileArgs {
    /// PIDs of GPU processes to collect; leave empty to auto-collect every process using a GPU
    #[arg(long, value_name = "Pid", required = false, num_args = 1..)]
    pub pids: Vec<i32>,

    /// Collection duration, in seconds
    #[arg(
        long,
        value_name = "Duration",
        required_unless_present = "iteration",
        default_value = "0"
    )]
    pub duration: u8,

    /// Iteration collection mode
    #[arg(
        long,
        value_name = "Iteration",
        required_unless_present = "duration",
        default_value = "false"
    )]
    pub iteration: bool,

    /// Number of iterations to collect
    #[arg(
        long,
        value_name = "NumSteps",
        required_if_eq("iteration", "true"),
        default_value = "0"
    )]
    pub num_steps: u32,

    /// Number of iterations to skip
    #[arg(
        long,
        value_name = "NumSkipSteps",
        required_if_eq("iteration", "true"),
        default_value = "0"
    )]
    pub num_skip_steps: u32,

    /// Entry module for iteration-mode collection
    #[arg(long, value_name = "IterationModule", default_value = "")]
    pub iteration_module: String,

    /// Entry function for iteration-mode collection
    #[arg(long, value_name = "IterationFunction", default_value = "")]
    pub iteration_function: String,

    /// Enable the python stack collector
    #[arg(long, value_name = "PyStack", default_value = "false", num_args(0..=1))]
    pub pystack: Option<bool>,

    /// Enable the Torch collector
    #[arg(long, value_name = "Torch", default_value = "false", num_args(0..=1))]
    pub torch: Option<bool>,

    /// Enable the GPU collector (on by default)
    #[arg(long, value_name = "Gpu", default_value = "true", num_args(0..=1))]
    pub gpu: Option<bool>,

    /// Argument shapes
    #[arg(long, value_name = "RecordShapes", default_value = "false", num_args(0..=1))]
    pub record_shapes: Option<bool>,

    /// Memory info
    #[arg(long, value_name = "Memory", default_value = "false", num_args(0..=1))]
    pub profile_memory: Option<bool>,

    /// FLOPS info
    #[arg(long, value_name = "Flops", default_value = "false", num_args(0..=1))]
    pub flops: Option<bool>,

    /// Module info
    #[arg(long, value_name = "Module", default_value = "false", num_args(0..=1))]
    pub with_modules: Option<bool>,

    /// PyTorch memory snapshot
    #[arg(long, value_name = "Snapshot", default_value = "false", num_args(0..=1))]
    pub snapshot: Option<bool>,

    /// Max traces retained in the memory snapshot; older entries are dropped on overflow. Default: duration*200k / iteration*1M
    #[arg(long, value_name = "MaxEntry", default_value = "0", num_args(0..=1))]
    pub snapshot_max_entry: Option<i32>,

    /// Output path
    #[arg(long, value_name = "Output", num_args(0..=1))]
    pub output: Option<String>,

    /// Whether to merge collected files
    #[arg(long, value_name = "Merge", default_value = "false", num_args(0..=1))]
    pub merge: Option<bool>,

    /// Config file directory
    #[arg(long, value_name = "ConfigPath", num_args(0..=1))]
    pub config_path: Option<String>,

    /// Enable default indicators: pystack, torch, gpu
    #[arg(long, value_name = "Adapt", default_value = "false", num_args(0..=1))]
    pub adapt: Option<bool>,

    /// Target process ID; used internally, not exposed on the command line
    #[arg(skip)]
    pub target_pid: i32,

    /// Enabled indicators
    #[arg(skip)]
    pub enable_indicator: Vec<IndicatorName>,

    /// Whether to upload to OSS
    #[arg(long, value_name = "Upload", default_value = "false", num_args(0..=1))]
    pub upload: Option<bool>,

    /// OSS Access Key
    #[arg(long, value_name = "Ak", required_if_eq("upload", "true"))]
    pub ak: Option<String>,

    /// OSS Secret Key
    #[arg(long, value_name = "Sk", required_if_eq("upload", "true"))]
    pub sk: Option<String>,

    /// OSS STS Token
    #[arg(long, value_name = "Sts", required_if_eq("upload", "true"))]
    pub sts: Option<String>,

    /// analysisId
    #[arg(long, value_name = "AnalysisId", required_if_eq("upload", "true"))]
    pub analysis_id: Option<String>,

    /// Data transport: oss=Aliyun OSS (default, compatible with --upload=true),
    /// dashboard=AIProf dashboard collector (POST tar.gz to /api/results/upload),
    /// sls=Aliyun Log Service (flatten chrome-trace JSON into LogEntry stream and PutLogs to the given logstore)
    #[arg(long, value_name = "Transport", value_enum, default_value = "oss")]
    pub transport: TransportKind,

    /// Collector endpoint for the dashboard transport, e.g. http://dashboard.svc:7000
    #[arg(
        long,
        value_name = "DashboardEndpoint",
        required_if_eq("transport", "dashboard")
    )]
    pub dashboard_endpoint: Option<String>,

    /// clientId reported by the dashboard transport; falls back to $CLIENT_ID / hostname
    #[arg(long, value_name = "ClientId")]
    pub client_id: Option<String>,

    /// Kubernetes namespace reported by the dashboard transport (optional; falls back to $NAMESPACE/$POD_NAMESPACE)
    #[arg(long, value_name = "Namespace")]
    pub namespace: Option<String>,

    /// Kubernetes pod name reported by the dashboard transport (optional; falls back to $POD_NAME)
    #[arg(long, value_name = "PodName")]
    pub pod_name: Option<String>,

    /// Kubernetes node name reported by the dashboard transport (optional; falls back to $NODE_NAME)
    #[arg(long, value_name = "NodeName")]
    pub node_name: Option<String>,

    /// SLS endpoint, e.g. cn-hangzhou.log.aliyuncs.com
    #[arg(
        long,
        value_name = "SlsEndpoint",
        required_if_eq("transport", "sls")
    )]
    pub sls_endpoint: Option<String>,

    /// SLS project name
    #[arg(
        long,
        value_name = "SlsProject",
        required_if_eq("transport", "sls")
    )]
    pub sls_project: Option<String>,

    /// SLS logstore name
    #[arg(
        long,
        value_name = "SlsLogstore",
        required_if_eq("transport", "sls")
    )]
    pub sls_logstore: Option<String>,

    /// SLS Access Key Id; falls back to $SLS_AK_ID
    #[arg(long, value_name = "SlsAkId")]
    pub sls_ak_id: Option<String>,

    /// SLS Access Key Secret; falls back to $SLS_AK_SECRET
    #[arg(long, value_name = "SlsAkSecret")]
    pub sls_ak_secret: Option<String>,

    /// SLS STS token (required for temporary credentials); falls back to $SLS_STS_TOKEN
    #[arg(long, value_name = "SlsStsToken")]
    pub sls_sts_token: Option<String>,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    /// Aliyun OSS multipart upload (original path); needs --upload true and AK/SK/STS env vars
    Oss,
    /// AIProf dashboard collector: package the output dir into tar.gz and POST via multipart
    Dashboard,
    /// Aliyun SLS: flatten already-written chrome-trace JSON into a LogEntry stream, PutLogs to SLS
    Sls,
}

impl Default for TransportKind {
    fn default() -> Self {
        TransportKind::Oss
    }
}

impl ProfileArgs {
    pub fn validate(&mut self) -> Result<(), ErrorCode> {
        let current_dir =
            env::current_dir().map_err(|e| ErrorCode::ParamsError(Some(e.to_string())))?;
        let exe_path =
            env::current_exe().map_err(|e| ErrorCode::ParamsError(Some(e.to_string())))?;

        // When adapt is true, enable the default indicators: pystack, torch, gpu
        if let Some(true) = self.adapt {
            self.pystack = Some(true);
            self.torch = Some(true);
            self.gpu = Some(true);
            // NOTE: with adapt on, if no pid is given, auto-collect every GPU-using process
            if self.pids.is_empty() {
                self.pids = list_gpu_processes().unwrap_or(vec![]);
            }
        }

        if self.pids.is_empty() {
            return Err(ErrorCode::ParamsError(Some("No processes using GPU were detected. Please enter the PID that needs to be collected".to_string())));
        }

        // Check the relationship between with_modules and torch
        if let Some(true) = self.with_modules {
            if !self.torch.unwrap_or(false) {
                self.torch = Some(true);
                log::warn!("Torch must be enabled when with_modules is set to true. Automatically set Torch to True!");
            }
        }

        // Check the other torch-dependent parameters
        if let Some(true) = self.record_shapes {
            if !self.torch.unwrap_or(false) {
                self.torch = Some(true);
                log::warn!("Torch must be enabled when record_shapes is set to true. Automatically set Torch to True!");
            }
        }

        if let Some(true) = self.profile_memory {
            self.torch = Some(true);
            if !self.torch.unwrap_or(false) {
                log::warn!("Torch must be enabled when profile_memory is set to true. Automatically set Torch to True!");
            }
        }

        if let Some(true) = self.flops {
            self.torch = Some(true);
            if !self.torch.unwrap_or(false) {
                log::warn!("Torch must be enabled when flops is set to true. Automatically set Torch to True!");
            }
        }

        // NOTE: input path does not exist
        if self.output.is_none() {
            self.output = Some(current_dir.to_string_lossy().to_string());
        } else {
            let output = self.output.clone().unwrap();
            let path = Path::new(&output);

            if path.exists() {
                self.output = Some(output);
            } else {
                self.output = Some("default".to_string());
                log::warn!(
                    "Output path does not exist: {}, we will output {}OFFSET.json",
                    output,
                    r#const::DEFAULT_PREFIX
                );
            }
        }

        if self.config_path.is_none() {
            self.config_path = Some(
                exe_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("config.yaml")
                    .to_string_lossy()
                    .to_string(),
            );
        }

        // Dashboard transport and --upload=true (OSS) take different paths; warn if both are given.
        if matches!(self.transport, TransportKind::Dashboard) {
            if self.upload.unwrap_or(false) {
                log::warn!(
                    "--upload=true is ignored when --transport=dashboard; results will be POSTed to the dashboard collector"
                );
            }
            if self.analysis_id.as_deref().unwrap_or("").trim().is_empty() {
                return Err(ErrorCode::ParamsError(Some(
                    "--analysis-id is required when --transport=dashboard (used as the collector taskId)".into(),
                )));
            }
        }

        // SLS transport: AK/SK/STS may come from env; analysis-id is reused as the group topic to filter by task.
        if matches!(self.transport, TransportKind::Sls) {
            if self.upload.unwrap_or(false) {
                log::warn!(
                    "--upload=true is ignored when --transport=sls; results will be PutLogs to SLS"
                );
            }
            if self.sls_ak_id.as_deref().unwrap_or("").trim().is_empty() {
                self.sls_ak_id = env::var("SLS_AK_ID").ok().filter(|s| !s.trim().is_empty());
            }
            if self.sls_ak_secret.as_deref().unwrap_or("").trim().is_empty() {
                self.sls_ak_secret =
                    env::var("SLS_AK_SECRET").ok().filter(|s| !s.trim().is_empty());
            }
            if self.sls_sts_token.as_deref().unwrap_or("").trim().is_empty() {
                self.sls_sts_token =
                    env::var("SLS_STS_TOKEN").ok().filter(|s| !s.trim().is_empty());
            }
            let dry_run = env::var("AIPROF_SLS_DRY_RUN")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            if !dry_run {
                if self.sls_ak_id.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(ErrorCode::ParamsError(Some(
                        "--sls-ak-id (or $SLS_AK_ID) is required when --transport=sls".into(),
                    )));
                }
                if self.sls_ak_secret.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(ErrorCode::ParamsError(Some(
                        "--sls-ak-secret (or $SLS_AK_SECRET) is required when --transport=sls".into(),
                    )));
                }
            }
            if self.analysis_id.as_deref().unwrap_or("").trim().is_empty() {
                return Err(ErrorCode::ParamsError(Some(
                    "--analysis-id is required when --transport=sls (used as the SLS topic)".into(),
                )));
            }
        }

        Ok(())
    }
}
