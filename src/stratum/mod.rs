pub mod error;
pub mod types;
pub mod v1;
pub mod miner;

use async_trait::async_trait;
use error::StratumError;
use types::*;

#[async_trait]
pub trait StratumClient: Send + Sync {
    /// Subscribe to the mining server
    async fn subscribe(&mut self) -> Result<SubscribeResponse, StratumError>;
    
    /// Authorize the mining client
    async fn authorize(&mut self, username: &str, password: &str) -> Result<AuthResponse, StratumError>;
    
    /// Submit a share to the mining server
    async fn submit_share(&mut self, share: Share) -> Result<bool, StratumError>;
    
    /// Get the current mining job
    async fn get_current_job(&mut self) -> Result<Option<MiningJob>, StratumError>;
    
    /// Handle incoming mining notifications
    async fn handle_notifications(&mut self) -> Result<(), StratumError>;
    
    /// Get the current mining target
    async fn get_target(&self) -> Result<MiningTarget, StratumError>;
    
    /// Get server information
    async fn get_server_info(&self) -> Result<ServerInfo, StratumError>;
    
    /// Reconnect to the mining server
    async fn reconnect(&mut self) -> Result<(), StratumError>;
    
    /// Close the connection
    async fn close(&mut self) -> Result<(), StratumError>;
}

/// Create a new Stratum client with the specified version
pub async fn create_client(
    version: StratumVersion,
    host: String,
    port: u16,
) -> Result<Box<dyn StratumClient>, StratumError> {
    match version {
        StratumVersion::V1 => {
            let client = v1::StratumV1Client::new(host, port).await?;
            Ok(Box::new(client) as Box<dyn StratumClient>)
        },
        StratumVersion::V2 => unimplemented!("Stratum V2 not yet implemented"),
    }
}
