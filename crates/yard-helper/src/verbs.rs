//! Dispatch for the fixed verb set. Anything else is `METHOD_NOT_FOUND`.

use crate::{HelperConfig, VERSION, resolver};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use yard_proto::helper::{self, HelperStatus, ResolverParams, TldParams};
use yard_proto::{ErrorCode, YardError};

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

    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, YardError> {
        match method {
            helper::RESOLVER_INSTALL => {
                let p: ResolverParams = params_of(params)?;
                to_value(resolver::install(&self.cfg.resolver_dir, &p.tld, p.port)?)
            }
            helper::RESOLVER_REMOVE => {
                let p: TldParams = params_of(params)?;
                to_value(resolver::remove(&self.cfg.resolver_dir, &p.tld)?)
            }
            helper::STATUS => to_value(HelperStatus { version: VERSION.into() }),
            _ => Err(YardError::new(ErrorCode::MethodNotFound, format!("the helper has no verb {method:?}"))),
        }
    }
}

fn params_of<T: DeserializeOwned>(params: Value) -> Result<T, YardError> {
    serde_json::from_value(params)
        .map_err(|e| YardError::new(ErrorCode::InvalidArgument, format!("bad parameters: {e}")))
}

fn to_value<T: Serialize>(value: T) -> Result<Value, YardError> {
    serde_json::to_value(value).map_err(YardError::internal)
}
