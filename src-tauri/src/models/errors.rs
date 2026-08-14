use crate::contracts::{BackendError, BackendErrorCode};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    ConfigLoadFailed,
    DownloadCancelled,
    InstallFailed,
    UnsupportedCapability,
    BackendMismatch,
    InvalidRequest,
    PreconditionFailed,
    OperationFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<BackendError>>,
}

impl CommandError {
    pub fn new(code: CommandErrorCode, message: String) -> Self {
        Self {
            code,
            message,
            details: None,
        }
    }
}

impl From<BackendError> for CommandError {
    fn from(error: BackendError) -> Self {
        let code = match error.code {
            BackendErrorCode::UnsupportedCapability => CommandErrorCode::UnsupportedCapability,
            BackendErrorCode::BackendMismatch => CommandErrorCode::BackendMismatch,
            BackendErrorCode::InvalidRequest => CommandErrorCode::InvalidRequest,
            BackendErrorCode::PreconditionFailed => CommandErrorCode::PreconditionFailed,
            BackendErrorCode::OperationFailed => CommandErrorCode::OperationFailed,
        };
        Self {
            code,
            message: error.message.clone(),
            details: Some(Box::new(error)),
        }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::new(CommandErrorCode::OperationFailed, message)
    }
}
