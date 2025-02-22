use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningJob {
    pub job_id: String,
    pub prev_hash: String,
    pub coinbase1: String,
    pub coinbase2: String,
    pub merkle_branch: Vec<String>,
    pub version: String,
    pub nbits: String,
    pub ntime: String,
    pub clean_jobs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeResponse {
    pub subscription_id: String,
    pub extranonce1: String,
    pub extranonce2_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub authorized: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Share {
    pub job_id: String,
    pub extranonce2: String,
    pub ntime: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StratumVersion {
    V1,
    V2,
}

impl fmt::Display for StratumVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StratumVersion::V1 => write!(f, "stratum+tcp"),
            StratumVersion::V2 => write!(f, "stratum2+tcp"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MiningTarget {
    pub difficulty: f64,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub connection_id: String,
}
