use crate::stratum::miner::Miner;
use crate::stratum::{error::StratumError, types::*};
use async_trait::async_trait;
use hex;
use rand::{thread_rng, Rng};
use serde_json::Value;
use std::sync::atomic::AtomicBool;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

const MAX_JOB_HISTORY: usize = 10;
const MAX_JOB_AGE: Duration = Duration::from_secs(600); // 10 minutes

#[derive(Debug, Clone)]
struct JobHistory {
    job: MiningJob,
    received_at: Instant,
}

#[derive(Debug)]
struct JobState {
    current_job: Option<MiningJob>,
    job_history: HashMap<String, JobHistory>,
    target: Option<MiningTarget>,
}

impl JobState {
    fn new() -> Self {
        Self {
            current_job: None,
            job_history: HashMap::with_capacity(MAX_JOB_HISTORY),
            target: None,
        }
    }

    fn add_job(&mut self, job: MiningJob) {
        // Add to history before updating current
        if job.clean_jobs {
            self.job_history.clear();
        }

        // Remove old jobs
        self.job_history
            .retain(|_, history| history.received_at.elapsed() < MAX_JOB_AGE);

        // Add new job to history
        if self.job_history.len() >= MAX_JOB_HISTORY {
            if let Some(oldest) = self
                .job_history
                .iter()
                .min_by_key(|(_, h)| h.received_at)
                .map(|(k, _)| k.clone())
            {
                self.job_history.remove(&oldest);
            }
        }

        self.job_history.insert(
            job.job_id.clone(),
            JobHistory {
                job: job.clone(),
                received_at: Instant::now(),
            },
        );

        // Update current job
        self.current_job = Some(job);
    }

    fn get_job(&self, job_id: &str) -> Option<&MiningJob> {
        self.job_history.get(job_id).map(|h| &h.job)
    }
}

/// Manages mining jobs and targets with validation and history tracking
#[derive(Clone)]
pub struct JobManager {
    job_from_stratum_tx: tokio::sync::mpsc::UnboundedSender<MiningJob>,
    is_alive: Arc<AtomicBool>,
    pub result_receiver: Arc<
        Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Result<(u32, MiningJob), StratumError>>>>,
    >,
    enqueued_job: Arc<Mutex<Option<MiningJob>>>,
    enqueued_difficulty: Arc<Mutex<Option<MiningTarget>>>,
    currently_running_job_id: Arc<Mutex<Option<String>>>,
    currently_running_merkle_root: Arc<Mutex<Option<Vec<String>>>>,
}

impl JobManager {
    /// Create a new job manager
    pub fn new<M: Miner>(miner: M) -> Self {
        let (job_from_stratum_tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MiningJob>();
        let (result_tx, result_receiver) = tokio::sync::mpsc::unbounded_channel();

        let currently_running_job_id = Arc::new(Mutex::new(None));
        let currently_running_job_id_clone = currently_running_job_id.clone();
        let currently_running_merkle_root = Arc::new(Mutex::new(None));
        let currently_running_merkle_root_clone = currently_running_merkle_root.clone();

        let is_alive = Arc::new(AtomicBool::new(true));
        let is_alive_clone = is_alive.clone();

        let background_worker = async move {
            if !is_alive_clone.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            let mut current_running_task_canceller = None;

            while let Some(job) = rx.recv().await {
                if current_running_task_canceller.take().is_some() {
                    log::warn!(target: "stratum", "Miner task cancelled because a newer job was received");
                }

                *currently_running_job_id_clone.lock().await = Some(job.job_id.clone());
                *currently_running_merkle_root_clone.lock().await = Some(job.merkle_branch.clone());

                let miner = miner.clone();
                let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
                current_running_task_canceller = Some(stop_tx);

                let result_tx = result_tx.clone();

                let currently_running_job_id_clone = currently_running_job_id_clone.clone();
                let currently_running_merkle_root_clone =
                    currently_running_merkle_root_clone.clone();

                let cancellable_task = tokio::spawn(async move {
                    let miner_task = miner.on_job_received(job);

                    tokio::select! {
                        _ = stop_rx => {
                            log::warn!(target: "stratum", "Miner task cancelled");
                        }
                        res = miner_task => {
                            if let Err(err) = result_tx.send(res) {
                                log::error!(target: "stratum", "Failed to send miner result: {err}");
                            }
                        }
                    }

                    let _ = currently_running_job_id_clone.lock().await.take();
                    let _ = currently_running_merkle_root_clone.lock().await.take();
                });

                drop(cancellable_task);
            }
        };

        tokio::spawn(background_worker);

        Self {
            job_from_stratum_tx,
            is_alive,
            result_receiver: Arc::new(Mutex::new(Some(result_receiver))),
            enqueued_job: Arc::new(Mutex::new(None)),
            enqueued_difficulty: Arc::new(Mutex::new(None)),
            currently_running_job_id,
            currently_running_merkle_root,
        }
    }

    /// Generate a random extranonce2 value of the specified size
    pub fn generate_extranonce2(size: usize) -> String {
        let mut rng = thread_rng();
        let mut bytes = vec![0u8; size];
        rng.fill(&mut bytes[..]);
        hex::encode(bytes)
    }

    /// Validate a mining job notification
    fn validate_job(params: &[Value]) -> Result<MiningJob, StratumError> {
        if params.len() < 8 {
            return Err(StratumError::InvalidJob("Incomplete job parameters".into()));
        }

        let job_id = params[0]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid job_id".into()))?
            .to_string();

        let prev_hash = params[1]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid prev_hash".into()))?;
        if prev_hash.len() != 64 {
            return Err(StratumError::InvalidJob(
                "prev_hash must be 32 bytes".into(),
            ));
        }

        let coinbase1 = params[2]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid coinbase1".into()))?;
        if hex::decode(coinbase1).is_err() {
            return Err(StratumError::InvalidJob(
                "coinbase1 must be hex encoded".into(),
            ));
        }

        let coinbase2 = params[3]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid coinbase2".into()))?;
        if hex::decode(coinbase2).is_err() {
            return Err(StratumError::InvalidJob(
                "coinbase2 must be hex encoded".into(),
            ));
        }

        let merkle_branch = params[4]
            .as_array()
            .ok_or_else(|| StratumError::InvalidJob("Invalid merkle_branch".into()))?
            .iter()
            .map(|v| v.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| StratumError::InvalidJob("Invalid merkle_branch format".into()))?;

        let version = params[5]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid version".into()))?;
        if version.len() != 8 {
            return Err(StratumError::InvalidJob("version must be 4 bytes".into()));
        }

        let nbits = params[6]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid nbits".into()))?;
        if nbits.len() != 8 {
            return Err(StratumError::InvalidJob("nbits must be 4 bytes".into()));
        }

        let ntime = params[7]
            .as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid ntime".into()))?;
        if ntime.len() != 8 {
            return Err(StratumError::InvalidJob("ntime must be 4 bytes".into()));
        }

        let clean_jobs = params.get(8).and_then(Value::as_bool).unwrap_or(false);

        Ok(MiningJob {
            job_id,
            prev_hash: prev_hash.to_string(),
            coinbase1: coinbase1.to_string(),
            coinbase2: coinbase2.to_string(),
            merkle_branch,
            version: version.to_string(),
            nbits: nbits.to_string(),
            ntime: ntime.to_string(),
            clean_jobs,
            target: None,
        })
    }

    /// Calculate target from difficulty
    fn calculate_target(difficulty: f64) -> [u8; 32] {
        let mut final_target = [0u8; 32];

        // For difficulty 1, target is:
        // 0x00000000ffff0000000000000000000000000000000000000000000000000000

        // Calculate mantissa with better precision
        let mantissa = (0xffff as f64 / difficulty) as u16;
        final_target[2] = (mantissa >> 8) as u8; // High byte
        final_target[3] = mantissa as u8; // Low byte

        final_target
    }

    /// Handle a new difficulty notification
    /// Step 1: Receive difficulty notification
    pub async fn handle_difficulty_notification(
        &self,
        params: &[Value],
    ) -> Result<(), StratumError> {
        if params.is_empty() {
            return Err(StratumError::Protocol(
                "Empty mining.set_difficulty params".into(),
            ));
        }

        let difficulty = params[0]
            .as_f64()
            .ok_or_else(|| StratumError::Protocol("Invalid difficulty value".into()))?;

        if difficulty <= 0.0 {
            return Err(StratumError::Protocol("Difficulty must be positive".into()));
        }

        let final_target = Self::calculate_target(difficulty);
        let mut lock = self.enqueued_difficulty.lock().await;
        *lock = Some(MiningTarget {
            difficulty,
            target: hex::encode(&final_target),
        });

        drop(lock);

        self.maybe_run_job().await
    }

    /// Handle a new job notification
    /// Step 2: Receive job, expect a difficulty notification
    pub async fn handle_job_notification(&self, params: &[Value]) -> Result<(), StratumError> {
        let job = Self::validate_job(params)?;
        let mut lock = self.enqueued_job.lock().await;
        *lock = Some(job.clone());
        drop(lock);

        // Now that we have received the difficulty, we can send the job to the background processor
        self.maybe_run_job().await
    }

    pub async fn maybe_run_job(&self) -> Result<(), StratumError> {
        // TODO: Refactor all this into a single Mutex wrapper
        let mut enqueued_job = self.enqueued_job.lock().await;
        let enqueued_difficulty = self.enqueued_difficulty.lock().await;
        let currently_running_job_id = self.currently_running_job_id.lock().await;
        let currently_running_merkle_root = self.currently_running_merkle_root.lock().await;

        match (enqueued_job.clone(), enqueued_difficulty.clone()) {
            (Some(mut job), Some(difficulty)) => {
                let job_ids_changed =
                    job.job_id != *currently_running_job_id.clone().unwrap_or_default();
                let merkle_root_changed =
                    job.merkle_branch != currently_running_merkle_root.clone().unwrap_or_default();
                let needs_to_run = job_ids_changed || merkle_root_changed;

                if needs_to_run {
                    job.target = Some(difficulty);
                    *enqueued_job = Some(job.clone());
                    log::info!(target: "stratum", "Execution criteria met. Running job: {job:?}");

                    self.job_from_stratum_tx.send(job).map_err(|err| {
                        StratumError::Io(format!(
                            "Failed to send job to job_from_stratum channel - {err}"
                        ))
                    })?;
                } else {
                    log::warn!(target: "stratum", "Job does not meet the criteria to run: job_ids_changed: {job_ids_changed}, merkle_root_changed: {merkle_root_changed}");
                }
            }

            _ => {
                log::warn!(target: "stratum", "Still waiting for both job and difficulty to be set ...");
            }
        }

        Ok(())
    }

    async fn get_job_or_error(&self) -> Result<MiningJob, StratumError> {
        self.enqueued_job
            .lock()
            .await
            .clone()
            .ok_or_else(|| StratumError::Protocol("No job available".into()))
    }

    /// Get the current mining job if available
    pub async fn get_current_job(&self) -> Result<Option<MiningJob>, StratumError> {
        Ok(self.enqueued_job.lock().await.clone())
    }

    /// Get the current target if available
    pub async fn get_target(&self) -> Result<MiningTarget, StratumError> {
        self.get_job_or_error()
            .await?
            .target
            .clone()
            .ok_or_else(|| StratumError::Protocol("No target set".into()))
    }

    /// Validate a share submission
    pub async fn validate_share(&self, share: &Share) -> Result<bool, StratumError> {
        let job = self.get_job_or_error().await?;

        // Validate nonce format
        if share.nonce.len() != 8 {
            return Err(StratumError::InvalidJob("Invalid nonce format".into()));
        }

        if hex::decode(&share.nonce).is_err() {
            return Err(StratumError::InvalidJob("Nonce must be hex encoded".into()));
        }

        // Validate extranonce2 format
        if hex::decode(&share.extranonce2).is_err() {
            return Err(StratumError::InvalidJob(
                "Extranonce2 must be hex encoded".into(),
            ));
        }

        // Validate ntime
        if share.ntime != job.ntime {
            return Err(StratumError::InvalidJob("Invalid ntime".into()));
        }

        // In a real implementation, we would:
        // 1. Reconstruct the block header
        // 2. Hash it
        // 3. Compare against target

        // For now, just validate formats
        Ok(true)
    }
}

#[derive(Copy, Clone)]
pub struct TestMiner;

#[async_trait]
impl Miner for TestMiner {
    async fn on_job_received(&self, job: MiningJob) -> Result<(u32, MiningJob), StratumError> {
        log::info!(target: "stratum", "Received job: {job:?}");
        tokio::time::sleep(Duration::from_millis(1000)).await;
        Ok((0, job))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_valid_job_params() -> Vec<Value> {
        vec![
            json!("job123"),                                                           // job_id
            json!("00000000000000000000000000000000000000000000000000000000deadbeef"), // prev_hash
            json!("01000000"),                                                         // coinbase1
            json!("02000000"),                                                         // coinbase2
            json!(["1234567890abcdef"]), // merkle_branch
            json!("00000001"),           // version
            json!("1d00ffff"),           // nbits
            json!("60509af9"),           // ntime
            json!(true),                 // clean_jobs
        ]
    }

    #[tokio::test]
    async fn test_job_validation() {
        let manager = JobManager::new(TestMiner);
        let params = create_valid_job_params();

        // Test valid job
        assert!(manager.handle_job_notification(&params).await.is_ok());

        // Test invalid job_id
        let mut invalid = params.clone();
        invalid[0] = json!(123); // Non-string job_id
        assert!(manager.handle_job_notification(&invalid).await.is_err());

        // Test invalid prev_hash
        let mut invalid = params.clone();
        invalid[1] = json!("invalid"); // Wrong length
        assert!(manager.handle_job_notification(&invalid).await.is_err());

        // Test missing parameters
        let invalid = params[..7].to_vec();
        assert!(manager.handle_job_notification(&invalid).await.is_err());
    }

    #[tokio::test]
    async fn test_difficulty_handling() {
        let manager = JobManager::new(TestMiner);

        let params = create_valid_job_params();
        // Test valid job
        assert!(manager.handle_job_notification(&params).await.is_ok());

        // Test valid difficulty
        assert!(manager
            .handle_difficulty_notification(&[json!(2.0)])
            .await
            .is_ok());

        // Test invalid difficulty
        assert!(manager
            .handle_difficulty_notification(&[json!(-1.0)])
            .await
            .is_err());
        assert!(manager
            .handle_difficulty_notification(&[json!(0.0)])
            .await
            .is_err());
        assert!(manager
            .handle_difficulty_notification(&[json!("invalid")])
            .await
            .is_err());
        assert!(manager.handle_difficulty_notification(&[]).await.is_err());

        // Verify target calculation
        manager
            .handle_difficulty_notification(&[json!(2.0)])
            .await
            .unwrap();
        let target = manager.get_target().await.unwrap();
        assert_eq!(target.difficulty, 2.0);

        // Target should be half of max target
        let _max_target = [0u8; 32];
        let actual_target = hex::decode(target.target).unwrap();
        assert_eq!(actual_target[2], 0x7f); // 0xff / 2
    }

    #[tokio::test]
    async fn test_share_validation() {
        let manager = JobManager::new(TestMiner);

        // Add a job
        let params = create_valid_job_params();
        manager.handle_job_notification(&params).await.unwrap();

        // Test valid share
        let share = Share {
            job_id: "job123".to_string(),
            extranonce2: "00000000".to_string(),
            ntime: "60509af9".to_string(),
            nonce: "00000000".to_string(),
        };
        assert!(manager.validate_share(&share).await.unwrap());

        // Test invalid nonce
        let invalid = Share {
            nonce: "invalid".to_string(),
            ..share.clone()
        };
        assert!(manager.validate_share(&invalid).await.is_err());

        // Test invalid ntime
        let invalid = Share {
            ntime: "00000000".to_string(),
            ..share
        };
        assert!(manager.validate_share(&invalid).await.is_err());
    }

    #[tokio::test]
    async fn test_generate_extranonce2() {
        let size = 4;
        let extranonce2 = JobManager::generate_extranonce2(size);
        assert_eq!(extranonce2.len(), size * 2); // Hex encoded
        assert!(hex::decode(&extranonce2).is_ok());
    }
}
