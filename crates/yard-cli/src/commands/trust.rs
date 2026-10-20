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
    let mut client = ctx.client().await?;
    let ca: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    if !ca.trusted {
        ctx.emit(&ca, || "The Yard CA is not in your login keychain.\n".into());
        return Ok(0);
    }
    let mut delete = Command::new(&ctx.security);
    delete.args(["delete-certificate", "-Z", &ca.sha256, "-t"]).arg(login_keychain()?);
    run(delete, &ctx.security).map_err(YardError::internal)?;
    let ca: CaInfo = client.call(methods::CA_INFO, Value::Null).await?;
    ctx.emit(&ca, || "Removed the Yard CA from your login keychain.\n".into());
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

fn login_keychain() -> Result<PathBuf, YardError> {
    let home = std::env::var_os("HOME").ok_or_else(|| YardError::internal("HOME is not set"))?;
    Ok(PathBuf::from(home).join("Library/Keychains/login.keychain-db"))
}
