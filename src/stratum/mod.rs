pub mod error;
pub mod miner;
pub mod types;
pub mod v1;

use crate::stratum::miner::Miner;
use async_trait::async_trait;
use error::StratumError;
use types::*;

#[async_trait]
pub trait StratumClient: Send + Sync + 'static {
    /// Subscribe to the mining server
    async fn subscribe(&mut self) -> Result<SubscribeResponse, StratumError>;

    /// Authorize the mining client
    async fn authorize(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthResponse, StratumError>;

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
pub async fn create_client<M: Miner>(
    version: StratumVersion,
    host: String,
    port: u16,
    miner: M,
) -> Result<Box<dyn StratumClient>, StratumError> {
    match version {
        StratumVersion::V1 => Ok(Box::new(v1::StratumV1Client::new(host, port, miner).await?)),
        StratumVersion::V2 => unimplemented!("Stratum V2 not yet implemented"),
    }
}
