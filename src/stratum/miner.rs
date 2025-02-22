use async_trait::async_trait;
use crate::stratum::{error::StratumError, types::MiningJob};

/// Trait for implementing mining functionality
#[async_trait]
pub trait Miner: Send + Sync {
    /// Called when a new mining job is received
    async fn on_job_received(&self, job: MiningJob) -> Result<u32, StratumError>;
}