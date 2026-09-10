// src/error.rs
use std::process::{ExitCode, Termination};

#[derive(Debug, PartialEq, Eq)]
pub enum ErrorCode {
    Success,
    NotFoundDetector(Option<String>),
    DetectEnvError(Option<String>),
    UnsupportedPlugin(Option<String>),
    MergeFileFailed(Option<String>),
    PykiPluginError(Option<String>),
    ConfigNotFound(Option<String>),
    CreateSchedulerError(Option<String>),
    InitCollectorError(Option<String>),
    TriggerCollectorError(Option<String>),
    ConfigParsingFailed(Option<String>),
    CommandFailed(Option<String>),
    PykiInstallFailed(Option<String>),
    PykiUninstallFailed(Option<String>),
    CopyFilesError(Option<String>),
    InjectFailed(Option<String>),
    SchedulerError(Option<String>),
    ParamsError(Option<String>),
    CppExceptionCaught(Option<String>),
    PermissionDenied(Option<String>),
    CuptiPluginError(Option<String>),
}

impl ErrorCode {
    pub fn code(&self) -> u8 {
        match self {
            ErrorCode::Success => 0,
            ErrorCode::NotFoundDetector(_) => 1,
            ErrorCode::DetectEnvError(_) => 2,
            ErrorCode::UnsupportedPlugin(_) => 4,
            ErrorCode::MergeFileFailed(_) => 5,
            ErrorCode::PykiPluginError(_) => 7,
            ErrorCode::ConfigNotFound(_) => 8,
            ErrorCode::CreateSchedulerError(_) => 9,
            ErrorCode::InitCollectorError(_) => 10,
            ErrorCode::TriggerCollectorError(_) => 11,
            ErrorCode::ConfigParsingFailed(_) => 14,
            ErrorCode::CommandFailed(_) => 15,
            ErrorCode::PykiInstallFailed(_) => 16,
            ErrorCode::PykiUninstallFailed(_) => 17,
            ErrorCode::CopyFilesError(_) => 18,
            ErrorCode::InjectFailed(_) => 19,
            ErrorCode::SchedulerError(_) => 20,
            ErrorCode::ParamsError(_) => 21,
            ErrorCode::CppExceptionCaught(_) => 22,
            ErrorCode::PermissionDenied(_) => 24,
            ErrorCode::CuptiPluginError(_) => 25,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            ErrorCode::Success => "Success",
            ErrorCode::NotFoundDetector(_) => "Detector Function not found.",
            ErrorCode::DetectEnvError(_) => "Fail to detect environment.",
            ErrorCode::UnsupportedPlugin(_) => "Unsupported plugin.",
            ErrorCode::MergeFileFailed(_) => "Failed to merge files.",
            ErrorCode::PykiPluginError(_) => "Not found pystack plugin function.",
            ErrorCode::ConfigNotFound(_) => "Not found config.yaml file.",
            ErrorCode::CreateSchedulerError(_) => "Fail to create collector scheduler.",
            ErrorCode::InitCollectorError(_) => "Fail to initialize collector.",
            ErrorCode::TriggerCollectorError(_) => "Failed to start the collector.",
            ErrorCode::ConfigParsingFailed(_) => "Failed to parse the config file.",
            ErrorCode::CommandFailed(_) => "Failed to execute the command.",
            ErrorCode::PykiInstallFailed(_) => "Failed to install PYKI.",
            ErrorCode::PykiUninstallFailed(_) => "Failed to uninstall PYKI.",
            ErrorCode::CopyFilesError(_) => "Copy failed.",
            ErrorCode::InjectFailed(_) => "Inject failed.",
            ErrorCode::SchedulerError(_) => "Scheduler inner error.",
            ErrorCode::ParamsError(_) => "Parameter verification failed.",
            ErrorCode::CppExceptionCaught(_) => "Caught C++ exception.",
            ErrorCode::PermissionDenied(_) => {
                "Permission denied, need root permission to run this program."
            }
            ErrorCode::CuptiPluginError(_) => "Not found cupti plugin function.",
        }
    }

    pub fn with_details<S: AsRef<str>>(self, details: S) -> Self {
        match self {
            ErrorCode::Success => ErrorCode::Success,
            ErrorCode::NotFoundDetector(_) => {
                ErrorCode::NotFoundDetector(Some(details.as_ref().to_string()))
            }
            ErrorCode::DetectEnvError(_) => {
                ErrorCode::DetectEnvError(Some(details.as_ref().to_string()))
            }
            ErrorCode::UnsupportedPlugin(_) => {
                ErrorCode::UnsupportedPlugin(Some(details.as_ref().to_string()))
            }
            ErrorCode::MergeFileFailed(_) => {
                ErrorCode::MergeFileFailed(Some(details.as_ref().to_string()))
            }
            ErrorCode::PykiPluginError(_) => {
                ErrorCode::PykiPluginError(Some(details.as_ref().to_string()))
            }
            ErrorCode::ConfigNotFound(_) => {
                ErrorCode::ConfigNotFound(Some(details.as_ref().to_string()))
            }
            ErrorCode::CreateSchedulerError(_) => {
                ErrorCode::CreateSchedulerError(Some(details.as_ref().to_string()))
            }
            ErrorCode::InitCollectorError(_) => {
                ErrorCode::InitCollectorError(Some(details.as_ref().to_string()))
            }
            ErrorCode::TriggerCollectorError(_) => {
                ErrorCode::TriggerCollectorError(Some(details.as_ref().to_string()))
            }
            ErrorCode::ConfigParsingFailed(_) => {
                ErrorCode::ConfigParsingFailed(Some(details.as_ref().to_string()))
            }
            ErrorCode::CommandFailed(_) => {
                ErrorCode::CommandFailed(Some(details.as_ref().to_string()))
            }
            ErrorCode::PykiInstallFailed(_) => {
                ErrorCode::PykiInstallFailed(Some(details.as_ref().to_string()))
            }
            ErrorCode::PykiUninstallFailed(_) => {
                ErrorCode::PykiUninstallFailed(Some(details.as_ref().to_string()))
            }
            ErrorCode::CopyFilesError(_) => {
                ErrorCode::CopyFilesError(Some(details.as_ref().to_string()))
            }
            ErrorCode::InjectFailed(_) => {
                ErrorCode::InjectFailed(Some(details.as_ref().to_string()))
            }
            ErrorCode::SchedulerError(_) => {
                ErrorCode::SchedulerError(Some(details.as_ref().to_string()))
            }
            ErrorCode::ParamsError(_) => ErrorCode::ParamsError(Some(details.as_ref().to_string())),
            ErrorCode::CppExceptionCaught(_) => {
                ErrorCode::CppExceptionCaught(Some(details.as_ref().to_string()))
            }
            ErrorCode::PermissionDenied(_) => {
                ErrorCode::PermissionDenied(Some(details.as_ref().to_string()))
            }
            ErrorCode::CuptiPluginError(_) => {
                ErrorCode::CuptiPluginError(Some(details.as_ref().to_string()))
            }
        }
    }

    pub fn details(&self) -> Option<&str> {
        match self {
            ErrorCode::Success => None,
            ErrorCode::NotFoundDetector(details) => details.as_deref(),
            ErrorCode::DetectEnvError(details) => details.as_deref(),
            ErrorCode::UnsupportedPlugin(details) => details.as_deref(),
            ErrorCode::MergeFileFailed(details) => details.as_deref(),
            ErrorCode::PykiPluginError(details) => details.as_deref(),
            ErrorCode::ConfigNotFound(details) => details.as_deref(),
            ErrorCode::CreateSchedulerError(details) => details.as_deref(),
            ErrorCode::InitCollectorError(details) => details.as_deref(),
            ErrorCode::TriggerCollectorError(details) => details.as_deref(),
            ErrorCode::ConfigParsingFailed(details) => details.as_deref(),
            ErrorCode::CommandFailed(details) => details.as_deref(),
            ErrorCode::PykiInstallFailed(details) => details.as_deref(),
            ErrorCode::PykiUninstallFailed(details) => details.as_deref(),
            ErrorCode::CopyFilesError(details) => details.as_deref(),
            ErrorCode::InjectFailed(details) => details.as_deref(),
            ErrorCode::SchedulerError(details) => details.as_deref(),
            ErrorCode::ParamsError(details) => details.as_deref(),
            ErrorCode::CppExceptionCaught(details) => details.as_deref(),
            ErrorCode::PermissionDenied(details) => details.as_deref(),
            ErrorCode::CuptiPluginError(details) => details.as_deref(),
        }
    }

    pub fn into_error(self) -> anyhow::Error {
        anyhow::Error::new(self)
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.details() {
            Some(details) => write!(f, "[{}] {} : {}", self.code(), self.message(), details),
            None => write!(f, "[{}] {}", self.code(), self.message()),
        }
    }
}

impl std::error::Error for ErrorCode {}

impl Termination for ErrorCode {
    fn report(self) -> ExitCode {
        ExitCode::from(self.code())
    }
}
