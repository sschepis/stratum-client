use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;

/// JSON-RPC request for Stratum protocol
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcRequest {
    pub id: u64,
    pub method: String,
    pub params: Vec<Value>,
}

/// JSON-RPC response for Stratum protocol
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcResponse {
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

/// Stratum V1 protocol methods
pub const MINING_SUBSCRIBE: &str = "mining.subscribe";
pub const MINING_AUTHORIZE: &str = "mining.authorize";
pub const MINING_SUBMIT: &str = "mining.submit";
pub const MINING_NOTIFY: &str = "mining.notify";
pub const MINING_SET_DIFFICULTY: &str = "mining.set_difficulty";

/// Client version string sent to pool
pub const CLIENT_VERSION: &str = "rust-stratum-client/1.0.0";

/// Default timeout for network operations in seconds
pub const DEFAULT_TIMEOUT: u64 = 20;

/// Maximum number of retries for failed operations
pub const MAX_RETRIES: u32 = 3;

impl JsonRpcRequest {
    /// Create a new request with the given method and parameters
    pub fn new(id: u64, method: impl Into<String>, params: Vec<Value>) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }

    /// Create a subscription request
    pub fn subscribe(id: u64) -> Self {
        Self::new(id, MINING_SUBSCRIBE, vec![json!(CLIENT_VERSION)])
    }

    /// Create an authorization request
    pub fn authorize(id: u64, username: &str, password: &str) -> Self {
        Self::new(id, MINING_AUTHORIZE, vec![json!(username), json!(password)])
    }

    /// Create a share submission request
    pub fn submit(id: u64, job_id: &str, extranonce2: &str, ntime: &str, nonce: &str) -> Self {
        Self::new(
            id,
            MINING_SUBMIT,
            vec![
                json!(job_id),
                json!(extranonce2),
                json!(ntime),
                json!(nonce),
            ],
        )
    }
}

impl fmt::Display for JsonRpcRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "JsonRpcRequest {{ id: {}, method: {}, params: {} }}",
            self.id,
            self.method,
            serde_json::to_string(&self.params).unwrap()
        )
    }
}

impl JsonRpcResponse {
    /// Create a new successful response
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create a new error response
    pub fn err(id: u64, error: Value) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Check if the response indicates success
    pub fn is_ok(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }

    /// Check if the response indicates an error
    pub fn is_err(&self) -> bool {
        self.error.is_some()
    }

    /// Get the error message if present
    pub fn error_message(&self) -> Option<String> {
        self.error
            .as_ref()
            .and_then(|e| e.as_str().map(String::from))
    }
}

impl fmt::Display for JsonRpcResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(error) = &self.error {
            write!(
                f,
                "JsonRpcResponse {{ id: {}, error: {} }}",
                self.id,
                serde_json::to_string(error).unwrap()
            )
        } else {
            write!(
                f,
                "JsonRpcResponse {{ id: {}, result: Some({}) }}",
                self.id,
                serde_json::to_string(self.result.as_ref().unwrap()).unwrap()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_creation() {
        let req = JsonRpcRequest::new(1, "test.method", vec![json!("param1")]);
        assert_eq!(req.id, 1);
        assert_eq!(req.method, "test.method");
        assert_eq!(req.params, vec![json!("param1")]);
    }

    #[test]
    fn test_subscribe_request() {
        let req = JsonRpcRequest::subscribe(1);
        assert_eq!(req.id, 1);
        assert_eq!(req.method, MINING_SUBSCRIBE);
        assert_eq!(req.params, vec![json!(CLIENT_VERSION)]);
    }

    #[test]
    fn test_authorize_request() {
        let req = JsonRpcRequest::authorize(1, "user", "pass");
        assert_eq!(req.id, 1);
        assert_eq!(req.method, MINING_AUTHORIZE);
        assert_eq!(req.params, vec![json!("user"), json!("pass")]);
    }

    #[test]
    fn test_submit_request() {
        let req = JsonRpcRequest::submit(1, "job1", "ext2", "time", "nonce");
        assert_eq!(req.id, 1);
        assert_eq!(req.method, MINING_SUBMIT);
        assert_eq!(
            req.params,
            vec![json!("job1"), json!("ext2"), json!("time"), json!("nonce")]
        );
    }

    #[test]
    fn test_response_ok() {
        let resp = JsonRpcResponse::ok(1, json!("result"));
        assert!(resp.is_ok());
        assert!(!resp.is_err());
        assert_eq!(resp.result, Some(json!("result")));
        assert_eq!(resp.error, None);
    }

    #[test]
    fn test_response_err() {
        let resp = JsonRpcResponse::err(1, json!("error"));
        assert!(!resp.is_ok());
        assert!(resp.is_err());
        assert_eq!(resp.result, None);
        assert_eq!(resp.error, Some(json!("error")));
        assert_eq!(resp.error_message(), Some("error".to_string()));
    }

    #[test]
    fn test_request_display() {
        let req = JsonRpcRequest::new(1, "test", vec![json!("param")]);
        assert_eq!(
            format!("{}", req),
            r#"JsonRpcRequest { id: 1, method: test, params: ["param"] }"#
        );
    }

    #[test]
    fn test_response_display() {
        let resp = JsonRpcResponse::ok(1, json!("result"));
        assert_eq!(
            format!("{}", resp),
            r#"JsonRpcResponse { id: 1, result: Some("result") }"#
        );

        let resp = JsonRpcResponse::err(1, json!("error"));
        assert_eq!(
            format!("{}", resp),
            r#"JsonRpcResponse { id: 1, error: "error" }"#
        );
    }
}
