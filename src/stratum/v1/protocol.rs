use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default timeout in seconds
pub const DEFAULT_TIMEOUT: u64 = 0; // No timeout - fail immediately
/// Default max retries
pub const MAX_RETRIES: u32 = 3;

/// Protocol constants
pub const CLIENT_VERSION: &str = "quantum-miner/0.1.0";
pub const MINING_SUBSCRIBE: &str = "mining.subscribe";
pub const MINING_AUTHORIZE: &str = "mining.authorize";
pub const MINING_NOTIFY: &str = "mining.notify";
pub const MINING_SET_DIFFICULTY: &str = "mining.set_difficulty";
pub const MINING_SUBMIT: &str = "mining.submit";

/// JSON-RPC request
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub id: u64,
    pub method: String,
    pub params: Vec<Value>,
}

impl JsonRpcRequest {
    /// Create a new request with the given method and parameters
    pub fn new(id: u64, method: impl Into<String>, params: Vec<Value>) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }

    /// Create a subscribe request
    pub fn subscribe(id: u64) -> Self {
        Self::new(id, MINING_SUBSCRIBE, vec![])
    }

    /// Create an authorize request
    pub fn authorize(id: u64, username: &str, password: &str) -> Self {
        Self::new(id, MINING_AUTHORIZE, vec![username.into(), password.into()])
    }

    /// Create a submit request
    pub fn submit(id: u64, job_id: &str, extranonce2: &str, ntime: &str, nonce: &str) -> Self {
        Self::new(
            id,
            MINING_SUBMIT,
            vec![
                job_id.into(),
                extranonce2.into(),
                ntime.into(),
                nonce.into(),
            ],
        )
    }
}

impl std::fmt::Display for JsonRpcRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JsonRpcRequest {{ id: {}, method: {}, params: {:?} }}",
            self.id, self.method, self.params
        )
    }
}

/// JSON-RPC response
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub id: u64,
    pub result: Option<Value>,
    pub error: Option<Value>,
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

    /// Returns true if the response indicates success
    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// Returns true if the response indicates an error
    pub fn is_err(&self) -> bool {
        self.error.is_some()
    }

    /// Returns the error message if present
    pub fn error_message(&self) -> Option<String> {
        self.error.as_ref().map(|e| e.to_string())
    }
}

impl std::fmt::Display for JsonRpcResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JsonRpcResponse {{ id: {}, result: {:?}, error: {:?} }}",
            self.id, self.result, self.error
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_creation() {
        let request = JsonRpcRequest::new(1, "test", vec![json!(42)]);
        assert_eq!(request.id, 1);
        assert_eq!(request.method, "test");
        assert_eq!(request.params, vec![json!(42)]);
    }

    #[test]
    fn test_subscribe_request() {
        let request = JsonRpcRequest::subscribe(1);
        assert_eq!(request.id, 1);
        assert_eq!(request.method, MINING_SUBSCRIBE);
        assert!(request.params.is_empty());
    }

    #[test]
    fn test_authorize_request() {
        let request = JsonRpcRequest::authorize(1, "user", "pass");
        assert_eq!(request.id, 1);
        assert_eq!(request.method, MINING_AUTHORIZE);
        assert_eq!(request.params, vec![json!("user"), json!("pass")]);
    }

    #[test]
    fn test_submit_request() {
        let request = JsonRpcRequest::submit(1, "job1", "ext2", "time", "nonce");
        assert_eq!(request.id, 1);
        assert_eq!(request.method, MINING_SUBMIT);
        assert_eq!(
            request.params,
            vec![
                json!("job1"),
                json!("ext2"),
                json!("time"),
                json!("nonce"),
            ]
        );
    }

    #[test]
    fn test_request_display() {
        let request = JsonRpcRequest::new(1, "test", vec![json!(42)]);
        assert_eq!(
            request.to_string(),
            "JsonRpcRequest { id: 1, method: test, params: [Number(42)] }"
        );
    }

    #[test]
    fn test_response_ok() {
        let response = JsonRpcResponse::ok(1, json!(true));
        assert_eq!(response.id, 1);
        assert_eq!(response.result, Some(json!(true)));
        assert_eq!(response.error, None);
        assert!(response.is_ok());
        assert!(!response.is_err());
        assert_eq!(response.error_message(), None);
    }

    #[test]
    fn test_response_err() {
        let response = JsonRpcResponse::err(1, json!("error"));
        assert_eq!(response.id, 1);
        assert_eq!(response.result, None);
        assert_eq!(response.error, Some(json!("error")));
        assert!(!response.is_ok());
        assert!(response.is_err());
        assert_eq!(response.error_message(), Some("\"error\"".to_string()));
    }

    #[test]
    fn test_response_display() {
        let response = JsonRpcResponse::ok(1, json!(true));
        assert_eq!(
            response.to_string(),
            "JsonRpcResponse { id: 1, result: Some(Bool(true)), error: None }"
        );
    }
}
