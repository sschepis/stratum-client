use crate::stratum::{error::StratumError, types::*};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use hex;
use rand::{thread_rng, Rng};

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
        self.job_history.retain(|_, history| {
            history.received_at.elapsed() < MAX_JOB_AGE
        });

        // Add new job to history
        if self.job_history.len() >= MAX_JOB_HISTORY {
            if let Some(oldest) = self.job_history
                .iter()
                .min_by_key(|(_, h)| h.received_at)
                .map(|(k, _)| k.clone())
            {
                self.job_history.remove(&oldest);
            }
        }

        self.job_history.insert(job.job_id.clone(), JobHistory {
            job: job.clone(),
            received_at: Instant::now(),
        });

        // Update current job
        self.current_job = Some(job);
    }

    fn get_job(&self, job_id: &str) -> Option<&MiningJob> {
        self.job_history.get(job_id).map(|h| &h.job)
    }
}

/// Manages mining jobs and targets with validation and history tracking
pub struct JobManager {
    state: Arc<Mutex<JobState>>,
}

impl JobManager {
    /// Create a new job manager
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(JobState::new())),
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

        let job_id = params[0].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid job_id".into()))?
            .to_string();

        let prev_hash = params[1].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid prev_hash".into()))?;
        if prev_hash.len() != 64 {
            return Err(StratumError::InvalidJob("prev_hash must be 32 bytes".into()));
        }

        let coinbase1 = params[2].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid coinbase1".into()))?;
        if hex::decode(coinbase1).is_err() {
            return Err(StratumError::InvalidJob("coinbase1 must be hex encoded".into()));
        }

        let coinbase2 = params[3].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid coinbase2".into()))?;
        if hex::decode(coinbase2).is_err() {
            return Err(StratumError::InvalidJob("coinbase2 must be hex encoded".into()));
        }

        let merkle_branch = params[4].as_array()
            .ok_or_else(|| StratumError::InvalidJob("Invalid merkle_branch".into()))?
            .iter()
            .map(|v| v.as_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| StratumError::InvalidJob("Invalid merkle_branch format".into()))?;

        let version = params[5].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid version".into()))?;
        if version.len() != 8 {
            return Err(StratumError::InvalidJob("version must be 4 bytes".into()));
        }

        let nbits = params[6].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid nbits".into()))?;
        if nbits.len() != 8 {
            return Err(StratumError::InvalidJob("nbits must be 4 bytes".into()));
        }

        let ntime = params[7].as_str()
            .ok_or_else(|| StratumError::InvalidJob("Invalid ntime".into()))?;
        if ntime.len() != 8 {
            return Err(StratumError::InvalidJob("ntime must be 4 bytes".into()));
        }

        let clean_jobs = params.get(8)
            .and_then(Value::as_bool)
            .unwrap_or(false);

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
        })
    }

    /// Calculate target from difficulty
    fn calculate_target(difficulty: f64) -> [u8; 32] {
        let mut final_target = [0u8; 32];
        
        // For difficulty 1, target is:
        // 0x00000000ffff0000000000000000000000000000000000000000000000000000
        
        // Calculate mantissa with better precision
        let mantissa = (0xffff as f64 / difficulty) as u16;
        final_target[2] = (mantissa >> 8) as u8;  // High byte
        final_target[3] = mantissa as u8;         // Low byte

        final_target
    }

    /// Handle a new job notification
    pub async fn handle_job_notification(&self, params: &[Value]) -> Result<(), StratumError> {
        let job = Self::validate_job(params)?;
        let mut state = self.state.lock().await;
        state.add_job(job);
        Ok(())
    }

    /// Handle a new difficulty notification
    pub async fn handle_difficulty_notification(&self, params: &[Value]) -> Result<(), StratumError> {
        if params.is_empty() {
            return Err(StratumError::Protocol("Empty mining.set_difficulty params".into()));
        }
        
        let difficulty = params[0].as_f64()
            .ok_or_else(|| StratumError::Protocol("Invalid difficulty value".into()))?;
        
        if difficulty <= 0.0 {
            return Err(StratumError::Protocol("Difficulty must be positive".into()));
        }

        let final_target = Self::calculate_target(difficulty);
        let mut state = self.state.lock().await;
        state.target = Some(MiningTarget {
            difficulty,
            target: hex::encode(&final_target),
        });

        Ok(())
    }

    /// Get the current mining job if available
    pub async fn get_current_job(&self) -> Result<Option<MiningJob>, StratumError> {
        Ok(self.state.lock().await.current_job.clone())
    }

    /// Get the current target if available
    pub async fn get_target(&self) -> Result<MiningTarget, StratumError> {
        self.state.lock().await.target.clone()
            .ok_or_else(|| StratumError::Protocol("No target available".into()))
    }

    /// Validate a share submission
    pub async fn validate_share(&self, share: &Share) -> Result<bool, StratumError> {
        let state = self.state.lock().await;
        
        // Get job from history
        let job = state.get_job(&share.job_id)
            .ok_or_else(|| StratumError::InvalidJob("Unknown job ID".into()))?;

        // Validate nonce format
        if share.nonce.len() != 8 {
            return Err(StratumError::InvalidJob("Invalid nonce format".into()));
        }
        if hex::decode(&share.nonce).is_err() {
            return Err(StratumError::InvalidJob("Nonce must be hex encoded".into()));
        }

        // Validate extranonce2 format
        if hex::decode(&share.extranonce2).is_err() {
            return Err(StratumError::InvalidJob("Extranonce2 must be hex encoded".into()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_valid_job_params() -> Vec<Value> {
        vec![
            json!("job123"),                      // job_id
            json!("00000000000000000000000000000000000000000000000000000000deadbeef"), // prev_hash
            json!("01000000"),                    // coinbase1
            json!("02000000"),                    // coinbase2
            json!(["1234567890abcdef"]),          // merkle_branch
            json!("00000001"),                    // version
            json!("1d00ffff"),                    // nbits
            json!("60509af9"),                    // ntime
            json!(true),                          // clean_jobs
        ]
    }

    #[tokio::test]
    async fn test_job_validation() {
        let manager = JobManager::new();
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
        let manager = JobManager::new();
        
        // Test valid difficulty
        assert!(manager.handle_difficulty_notification(&[json!(2.0)]).await.is_ok());
        
        // Test invalid difficulty
        assert!(manager.handle_difficulty_notification(&[json!(-1.0)]).await.is_err());
        assert!(manager.handle_difficulty_notification(&[json!(0.0)]).await.is_err());
        assert!(manager.handle_difficulty_notification(&[json!("invalid")]).await.is_err());
        assert!(manager.handle_difficulty_notification(&[]).await.is_err());

        // Verify target calculation
        manager.handle_difficulty_notification(&[json!(2.0)]).await.unwrap();
        let target = manager.get_target().await.unwrap();
        assert_eq!(target.difficulty, 2.0);
        
        // Target should be half of max target
        let _max_target = [0u8; 32];
        let actual_target = hex::decode(target.target).unwrap();
        assert_eq!(actual_target[2], 0x7f); // 0xff / 2
    }

    #[tokio::test]
    async fn test_job_history() {
        let manager = JobManager::new();
        let mut params = create_valid_job_params();

        // Add multiple jobs
        for i in 0..15 {
            params[0] = json!(format!("job{}", i));
            manager.handle_job_notification(&params).await.unwrap();
        }

        // Verify we can only get recent jobs
        let state = manager.state.lock().await;
        assert!(state.job_history.len() <= MAX_JOB_HISTORY);
        
        // Verify clean_jobs behavior
        params[0] = json!("new_job");
        params[8] = json!(true); // clean_jobs = true
        drop(state);
        
        manager.handle_job_notification(&params).await.unwrap();
        let state = manager.state.lock().await;
        assert_eq!(state.job_history.len(), 1);
        assert!(state.get_job("new_job").is_some());
    }

    #[tokio::test]
    async fn test_share_validation() {
        let manager = JobManager::new();
        
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

        // Test unknown job
        let invalid = Share {
            job_id: "unknown".to_string(),
            ..share.clone()
        };
        assert!(manager.validate_share(&invalid).await.is_err());

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
