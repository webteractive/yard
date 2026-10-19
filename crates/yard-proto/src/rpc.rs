//! JSON-RPC 2.0 envelopes, framed as one JSON object per line.

use crate::{ErrorCode, Event, YardError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC: &str = "2.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Self { jsonrpc: JSONRPC.into(), id: Some(id), method: method.into(), params }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: YardError,
}

impl From<YardError> for RpcError {
    fn from(e: YardError) -> Self {
        let code = match e.code {
            ErrorCode::InvalidRequest => -32600,
            ErrorCode::MethodNotFound => -32601,
            _ => -32000,
        };
        Self { code, message: e.message.clone(), data: e }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self { jsonrpc: JSONRPC.into(), id, result: Some(result), error: None }
    }

    pub fn err(id: u64, error: YardError) -> Self {
        Self { jsonrpc: JSONRPC.into(), id, result: None, error: Some(error.into()) }
    }

    pub fn into_result(self) -> Result<Value, YardError> {
        match self.error {
            Some(e) => Err(e.data),
            None => Ok(self.result.unwrap_or(Value::Null)),
        }
    }
}

/// A server push: always method `event`, never answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Event,
}

impl Notification {
    pub fn event(event: Event) -> Self {
        Self { jsonrpc: JSONRPC.into(), method: "event".into(), params: event }
    }
}

/// Serializes `value` as a single line ending in `\n`. serde_json escapes newlines
/// inside strings, so the only raw newline is the terminator.
pub fn encode_line<T: Serialize>(value: &T) -> Vec<u8> {
    let mut line = serde_json::to_vec(value).unwrap_or_else(|e| {
        serde_json::to_vec(&YardError::internal(format!("unserializable message: {e}"))).unwrap_or_default()
    });
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ErrorCode, YardError};

    #[test]
    fn encode_line_ends_with_single_newline() {
        let req = Request::new(7, "status", serde_json::json!({"a": "x\ny"}));
        let line = encode_line(&req);
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);
        let back: Request = serde_json::from_slice(&line).unwrap();
        assert_eq!(back.method, "status");
        assert_eq!(back.jsonrpc, "2.0");
    }

    #[test]
    fn yard_error_maps_to_rpc_minus_32000() {
        let e = YardError::new(ErrorCode::SiteNotFound, "no site blog");
        let r = RpcError::from(e.clone());
        assert_eq!(r.code, -32000);
        assert_eq!(r.message, e.message);
        assert_eq!(r.data, e);
    }

    #[test]
    fn protocol_errors_use_standard_codes() {
        assert_eq!(RpcError::from(YardError::new(ErrorCode::MethodNotFound, "x")).code, -32601);
        assert_eq!(RpcError::from(YardError::new(ErrorCode::InvalidRequest, "x")).code, -32600);
    }

    #[test]
    fn response_carries_result_or_error() {
        let ok = Response::ok(1, serde_json::json!({"x": 1}));
        assert!(ok.error.is_none());
        let err = Response::err(2, YardError::new(ErrorCode::Internal, "boom"));
        assert!(err.result.is_none());
        assert_eq!(err.into_result().unwrap_err().code, ErrorCode::Internal);
    }
}
