use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandErrorCode {
    ConfigLoadFailed,
    DownloadCancelled,
    InstallFailed,
    OperationFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: CommandErrorCode,
    pub message: String,
}

impl CommandError {
    pub fn new(code: CommandErrorCode, message: String) -> Self {
        Self { code, message }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::new(CommandErrorCode::OperationFailed, message)
    }
}
