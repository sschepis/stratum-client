use crate::stratum::error::StratumError;
use crate::stratum::types::MiningJob;
use async_trait::async_trait;

#[async_trait]
pub trait Miner: Clone + Send + 'static {
    async fn on_job_received(&self, job: MiningJob) -> Result<(u32, MiningJob), StratumError>;
}
