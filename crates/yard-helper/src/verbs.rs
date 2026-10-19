//! Dispatch for the fixed verb set. Anything else is `METHOD_NOT_FOUND`.

use crate::nginx::Nginx;
use crate::{HelperConfig, VERSION, resolver};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Mutex;
use yard_core::Paths;
use yard_core::brew::Brew;
use yard_proto::helper::{self, HelperStatus, ResolverParams, TldParams};
use yard_proto::{ErrorCode, YardError};

pub struct Helper {
    cfg: HelperConfig,
    nginx: Nginx,
    /// One nginx verb at a time, so a stop never races a start.
    nginx_lock: Mutex<()>,
}

impl Helper {
    pub fn new(cfg: HelperConfig) -> Self {
        let nginx = Nginx::new(&Paths::new(&cfg.root), &Brew::new(&cfg.brew_prefix));
        Self { cfg, nginx, nginx_lock: Mutex::new(()) }
    }

    pub fn owner_uid(&self) -> u32 {
        self.cfg.uid
    }

    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, YardError> {
        match method {
            helper::NGINX_START => {
                let _one = self.nginx_lock.lock().await;
                to_value(self.nginx.start().await?)
            }
            helper::NGINX_STOP => {
                let _one = self.nginx_lock.lock().await;
                to_value(self.nginx.stop().await?)
            }
            helper::NGINX_RELOAD => {
                let _one = self.nginx_lock.lock().await;
                to_value(self.nginx.reload()?)
            }
            helper::NGINX_TEST_CONFIG => to_value(self.nginx.test_config().await?),
            helper::RESOLVER_INSTALL => {
                let p: ResolverParams = params_of(params)?;
                to_value(resolver::install(&self.cfg.resolver_dir, &p.tld, p.port)?)
            }
            helper::RESOLVER_REMOVE => {
                let p: TldParams = params_of(params)?;
                to_value(resolver::remove(&self.cfg.resolver_dir, &p.tld)?)
            }
            helper::STATUS => {
                to_value(HelperStatus { version: VERSION.into(), nginx_pid: self.nginx.running_pid() })
            }
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
