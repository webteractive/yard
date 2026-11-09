//! Dispatch for the fixed verb set. Anything else is `METHOD_NOT_FOUND`.

use crate::{HELPER_REVISION, HelperConfig, VERSION, ports, resolver};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::os::fd::OwnedFd;
use yard_proto::helper::{self, BindParams, BindResult, HelperStatus, ResolverParams, TldParams};
use yard_proto::{ErrorCode, YardError};

/// A verb's answer, and the descriptors that travel with it (only `ports.bind` has any).
pub struct Reply {
    pub result: Value,
    pub fds: Vec<OwnedFd>,
}

impl Reply {
    fn of<T: Serialize>(value: T) -> Result<Self, YardError> {
        Ok(Self { result: serde_json::to_value(value).map_err(YardError::internal)?, fds: vec![] })
    }
}

pub struct Helper {
    cfg: HelperConfig,
}

impl Helper {
    pub fn new(cfg: HelperConfig) -> Self {
        Self { cfg }
    }

    pub fn owner_uid(&self) -> u32 {
        self.cfg.uid
    }

    pub async fn handle(&self, method: &str, params: Value) -> Result<Reply, YardError> {
        match method {
            helper::RESOLVER_INSTALL => {
                let p: ResolverParams = params_of(params)?;
                Reply::of(resolver::install(&self.cfg.resolver_dir, &p.tld, p.port)?)
            }
            helper::RESOLVER_REMOVE => {
                let p: TldParams = params_of(params)?;
                Reply::of(resolver::remove(&self.cfg.resolver_dir, &p.tld)?)
            }
            helper::PORTS_BIND => {
                let BindParams { port } = params_of(params)?;
                self.bind(port)
            }
            helper::STATUS => Reply::of(HelperStatus { version: VERSION.into(), revision: HELPER_REVISION }),
            _ => Err(YardError::new(ErrorCode::MethodNotFound, format!("the helper has no verb {method:?}"))),
        }
    }

    /// Only the configured privileged ports; anything else the owner can bind itself.
    fn bind(&self, port: u16) -> Result<Reply, YardError> {
        if !self.cfg.privileged_ports.contains(&port) {
            let allowed: Vec<String> = self.cfg.privileged_ports.iter().map(u16::to_string).collect();
            return Err(YardError::new(
                ErrorCode::InvalidArgument,
                format!("ports.bind opens only ports {}, not {port}", allowed.join(" and ")),
            ));
        }
        let fds = ports::bind_loopback(port)?;
        let addresses = ports::loopback(port).iter().map(ToString::to_string).collect();
        Ok(Reply { fds, ..Reply::of(BindResult { addresses })? })
    }
}

fn params_of<T: DeserializeOwned>(params: Value) -> Result<T, YardError> {
    serde_json::from_value(params)
        .map_err(|e| YardError::new(ErrorCode::InvalidArgument, format!("bad parameters: {e}")))
}
