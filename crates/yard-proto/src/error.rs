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
    /// `yard trust` ran `security`, but the login keychain still does not trust the CA.
    CaNotTrusted,
    /// `yard untrust` ran `security`, but the CA is still trusted, for instance by trust
    /// settings outside the login keychain.
    CaStillTrusted,
    /// A setting the site's `.yard` owns was to be changed by a command; edit the file.
    SiteHasProjectFile,
    /// A `.yard` needs something installed and nobody said yes; `data.needs` lists it.
    NeedsInstall,
    /// A `.yard` does not parse or has an unknown key; `data` has the file and line.
    ProjectFileInvalid,
    /// `yard init` would take a name another directory or proxy already serves;
    /// `data` has `name` and `serves`. `replace` moves that site here.
    SiteNameTaken,
    /// No service instance has that name; `yard services` lists them.
    ServiceNotFound,
    /// That service instance is already added.
    ServiceExists,
    /// A site's `.yard` lists the instance, so removing it needs confirmation or `--yes`;
    /// `data.sites` names them.
    ServiceInUse,
    /// The engine has no such line in brew; `data.available` lists the lines it has.
    ServiceVersionUnsupported,
    /// Homebrew does not trust the formula's tap (`brew trust`); `data` names the tap and formula.
    FormulaUntrusted,
    /// The command removes data or trusts a source and needs a yes it did not get.
    NeedsConfirmation,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        assert_eq!(serde_json::to_value(ErrorCode::CaNotTrusted).unwrap(), json!("CA_NOT_TRUSTED"));
        assert_eq!(serde_json::to_value(ErrorCode::CaStillTrusted).unwrap(), json!("CA_STILL_TRUSTED"));
        assert_eq!(
            serde_json::to_value(ErrorCode::SiteHasProjectFile).unwrap(),
            json!("SITE_HAS_PROJECT_FILE")
        );
        assert_eq!(serde_json::to_value(ErrorCode::NeedsInstall).unwrap(), json!("NEEDS_INSTALL"));
        assert_eq!(serde_json::to_value(ErrorCode::SiteNameTaken).unwrap(), json!("SITE_NAME_TAKEN"));
        assert_eq!(
            serde_json::to_value(ErrorCode::ProjectFileInvalid).unwrap(),
            json!("PROJECT_FILE_INVALID")
        );
        for (code, name) in [
            (ErrorCode::ServiceNotFound, "SERVICE_NOT_FOUND"),
            (ErrorCode::ServiceExists, "SERVICE_EXISTS"),
            (ErrorCode::ServiceInUse, "SERVICE_IN_USE"),
            (ErrorCode::ServiceVersionUnsupported, "SERVICE_VERSION_UNSUPPORTED"),
            (ErrorCode::FormulaUntrusted, "FORMULA_UNTRUSTED"),
            (ErrorCode::NeedsConfirmation, "NEEDS_CONFIRMATION"),
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), json!(name));
        }
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
