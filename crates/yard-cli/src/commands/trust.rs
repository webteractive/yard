//! `yard trust` and `untrust`: Yard's CA in the login keychain, through macOS's `security`
//! run as the user, which asks for the password itself (spec §5.4).

use super::{Ctx, Outcome};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use yard_proto::methods;
use yard_proto::{CaInfo, ErrorCode, YardError};

pub async fn trust(ctx: &Ctx) -> Outcome {
    let mut client = ctx.client().await?;
    let ca: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    if ca.trusted {
        ctx.emit(&ca, || "The Yard CA is already trusted.\n".into());
        return Ok(0);
    }
    let not_trusted =
        |message: String| YardError::new(ErrorCode::CaNotTrusted, message).with_hint("yard trust");
    let mut add = Command::new(&ctx.security);
    add.args(["add-trusted-cert", "-r", "trustRoot", "-k"]).arg(login_keychain()?).arg(&ca.cert);
    run(add, &ctx.security).map_err(not_trusted)?;
    let ca: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    if !ca.trusted {
        return Err(not_trusted("the CA is still not trusted".into()).into());
    }
    ctx.emit(&ca, || "Trusted the Yard CA in your login keychain.\n".into());
    Ok(0)
}

pub async fn untrust(ctx: &Ctx) -> Outcome {
    // ca.info would make a CA just to say it is not trusted.
    if !ctx.paths.ca_cert().exists() {
        ctx.emit(&Value::Null, || "There is no Yard CA, so there is nothing to untrust.\n".into());
        return Ok(0);
    }
    let mut client = ctx.client().await?;
    let ca: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    if !ca.trusted {
        ctx.emit(&ca, || "The Yard CA is not in your login keychain.\n".into());
        return Ok(0);
    }
    let mut delete = Command::new(&ctx.security);
    delete.args(["delete-certificate", "-Z", &ca.sha256, "-t"]).arg(login_keychain()?);
    // security fails when the certificate is not in the login keychain at all; whether
    // the CA is still trusted is what decides.
    let deleted = run(delete, &ctx.security);
    let after: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    if after.trusted {
        let message = match deleted {
            Ok(()) => "the CA is still trusted: its trust is set outside your login keychain".to_string(),
            Err(e) => format!("{e}, and the CA is still trusted"),
        };
        let hint = format!("sudo security remove-trusted-cert -d {}", after.cert);
        return Err(YardError::new(ErrorCode::CaStillTrusted, message).with_hint(hint).into());
    }
    ctx.emit(&after, || "Removed the Yard CA from your login keychain.\n".into());
    Ok(0)
}

/// `security` on the terminal, so its password prompt reaches the user.
fn run(mut command: Command, program: &std::path::Path) -> Result<(), String> {
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("security exited with {}", code(status))),
        Err(e) => Err(format!("cannot run {}: {e}", program.display())),
    }
}

fn code(status: ExitStatus) -> String {
    status.code().map_or_else(|| status.to_string(), |c| c.to_string())
}

/// Removes the CA from the login keychain without yardd (for `yard uninstall`, which may run
/// with yardd stopped): the fingerprint comes from the certificate file, and the trust check
/// from `security` itself.
pub async fn untrust_offline(ctx: &Ctx) -> Result<(), YardError> {
    let ca = ctx.paths.ca_cert();
    let sha = yard_daemon::tls::ca_sha256(&ctx.paths)?;
    let mut delete = Command::new(&ctx.security);
    delete.args(["delete-certificate", "-Z", &sha, "-t"]).arg(login_keychain()?);
    let deleted = run(delete, &ctx.security);
    if yard_daemon::tls::trusted(&ctx.security, &ca).await == Some(true) {
        let why = deleted.err().unwrap_or_else(|| "its trust is set outside your login keychain".into());
        return Err(YardError::new(ErrorCode::CaStillTrusted, format!("the CA is still trusted: {why}"))
            .with_hint(format!("sudo security remove-trusted-cert -d {}", ca.display())));
    }
    Ok(())
}

fn login_keychain() -> Result<PathBuf, YardError> {
    let home = std::env::var_os("HOME").ok_or_else(|| YardError::internal("HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/Keychains/login.keychain-db"))
}
