//! The error every Yard API call and CLI command reports (spec §6.3).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Stable error codes. Never rename a variant: clients branch on the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ConfigInvalid,
    PortInUse,
    PortConflict,
    BrewMissing,
    FormulaMissing,
    PhpNotInstalled,
    /// A PHP version is the default or a site's, so it cannot be removed.
    PhpInUse,
    NginxConfigInvalid,
    HelperNotInstalled,
    HelperRefused,
    SiteNotFound,
    InvalidArgument,
    ResolverForeign,
    ProtocolMismatch,
    MethodNotFound,
    InvalidRequest,
    /// Reported by the CLI when `yardd` does not answer on its socket.
    DaemonNotRunning,
    /// A command a job ran (brew, pecl) exited non-zero; `data.tail` has its last lines.
    JobFailed,
    /// A job was cancelled (`jobs.cancel`, or Ctrl-C in the CLI) before it finished.
    JobCancelled,
    /// No queued or running job has that id: it never existed, or it has finished.
    JobNotFound,
    /// An extension installed but `php -m` does not list it; `data.output` has its output.
    ExtVerifyFailed,
    /// `yard.toml` manages no extension of that name for the version, and brew loads none.
    ExtNotFound,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YardError {
    pub code: ErrorCode,
    pub message: String,
    /// The exact command or step that fixes it, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl YardError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None, data: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn internal(message: impl fmt::Display) -> Self {
        Self::new(ErrorCode::Internal, message.to_string())
    }
}

impl fmt::Display for YardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for YardError {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_serializes_with_string_code() {
        let e = YardError::new(ErrorCode::PortInUse, "port 8080 is in use")
            .with_hint("yard ports set nginx-http <port>");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"code":"PORT_IN_USE","message":"port 8080 is in use","hint":"yard ports set nginx-http <port>"})
        );
    }

    #[test]
    fn daemon_not_running_has_a_stable_code() {
        let v = serde_json::to_value(ErrorCode::DaemonNotRunning).unwrap();
        assert_eq!(v, json!("DAEMON_NOT_RUNNING"));
    }

    #[test]
    fn error_round_trips_with_data() {
        let e = YardError::new(ErrorCode::ConfigInvalid, "bad toml")
            .with_data(json!({"file": "yard.toml", "line": 2}));
        let back: YardError = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        assert_eq!(e.to_string(), "bad toml");
    }
}
