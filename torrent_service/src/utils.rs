use thiserror::Error;
use serde::{Deserialize, Serialize};

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] bincode::Error),
    #[error("Authentication error: {0}")]
    AuthError(String),
    #[error("Alert error: {0}")]
    AlertError(String),
    #[error("Torrent error: {0}")]
    Torrent(String),
    // Add more
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub piece_size: usize,
    pub aged_threshold: AgedThreshold,
    pub stake_amount: u64,
    pub proof_reward_base: u64,
    pub bulk_reward_per_mb: u64,
    // Add ports, etc.
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgedThreshold {
    Months(u32),
    Blocks(u64),
}
