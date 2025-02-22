use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum StratumError {
    #[error("JSON error: {0}")]
    Json(String),
    
    #[error("IO error: {0}")]
    Io(String),

    #[error("Hex decode error: {0}")]
    HexDecode(String),
    
    #[error("Protocol error: {0}")]
    Protocol(String),
    
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    
    #[error("Subscription failed: {0}")]
    SubscriptionFailed(String),
    
    #[error("Invalid job received: {0}")]
    InvalidJob(String),
    
    #[error("Connection error: {0}")]
    Connection(String),

}

impl From<std::io::Error> for StratumError {
    fn from(err: std::io::Error) -> Self {
        StratumError::Io(err.to_string())
    }
}

impl From<serde_json::Error> for StratumError {
    fn from(err: serde_json::Error) -> Self {
        StratumError::Json(err.to_string())
    }
}

impl From<hex::FromHexError> for StratumError {
    fn from(err: hex::FromHexError) -> Self {
        StratumError::HexDecode(err.to_string())
    }
}
