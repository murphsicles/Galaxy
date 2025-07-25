// torrent_service/src/incentives.rs
use crate::utils::{Config, ServiceError};
use crate::tracker::TrackerManager;
use sv::transaction::Transaction as SvTx;
use sv::script::{Script, Opcode};
use sv::keys::{PrivateKey, PublicKey};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use tracing::{info, error};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use toml;

pub struct IncentivesManager {
    config: Config,
    transaction_service_addr: String,
    tracker: Arc<TrackerManager>,
    auth_token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxRequest {
    tx: SvTx,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxResponse {
    success: bool,
    error: String,
}

impl IncentivesManager {
    pub fn new(config: &Config) -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let toml_config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let auth_token = toml_config.get("torrent_service").and_then(|s| s.get("auth_token").and_then(|v| v.as_str())).unwrap_or("default_token").to_string();

        Self {
            config: config.clone(),
            transaction_service_addr: "127.0.0.1:50052".to_string(),
            tracker: Arc::new(TrackerManager::new(config).await),
            auth_token,
        }
    }

    pub async fn reward_proof(&self, user_id: &str, block_hash: &str) -> Result<(), ServiceError> {
        let start = Instant::now();
        let reward = self.config.proof_reward_base; // 100 sat base
        let bonus = self.calculate_bonus(block_hash, start).await; // Calculate speed and rarity bonus
        let total_reward = reward + bonus;

        let tx = self.build_op_return_tx(user_id, total_reward, "proof_reward").await?;
        self.broadcast_tx(&tx).await?;
        info!("Proof reward of {} sat (base: {}, bonus: {}) sent to user_id: {}", total_reward, reward, bonus, user_id);
        Ok(())
    }

    pub async fn reward_bulk(&self, user_id: &str, mb_transferred: u64) -> Result<(), ServiceError> {
        let reward = mb_transferred * self.config.bulk_reward_per_mb; // 100 sat/MB
        let tx = self.build_op_return_tx(user_id, reward, "bulk_reward").await?;
        self.broadcast_tx(&tx).await?;
        info!("Bulk reward of {} sat sent to user_id: {} for {} MB", reward, user_id, mb_transferred);
        Ok(())
    }

    pub async fn stake(&self, user_id: &str, amount: u64) -> Result<(), ServiceError> {
        if amount < self.config.stake_amount {
            return Err(ServiceError::IncentiveError(format!("Stake {} sat below required {}", amount, self.config.stake_amount)));
        }
        let tx = self.build_op_return_tx(user_id, amount, "stake").await?;
        self.broadcast_tx(&tx).await?;
        info!("Stake of {} sat accepted for user_id: {}", amount, user_id);
        Ok(())
    }

    pub async fn slash(&self, user_id: &str, amount: u64) -> Result<(), ServiceError> {
        let tx = self.build_op_return_tx(user_id, amount, "slash").await?;
        self.broadcast_tx(&tx).await?;
        info!("Slashed {} sat from user_id: {}", amount, user_id);
        Ok(())
    }

    async fn build_op_return_tx(&self, user_id: &str, amount: u64, action: &str) -> Result<SvTx, ServiceError> {
        let priv_key = PrivateKey::from_random();
        let pub_key = PublicKey::from_private_key(&priv_key);

        let mut script = Script::new();
        script.append(Opcode::OP_RETURN);
        script.append_data(action.as_bytes());
        script.append_data(user_id.as_bytes());

        let mut tx = SvTx::new();
        tx.add_output(amount, &script);
        // Placeholder: Add input from wallet (requires actual utxos)
        // tx.add_input(...);

        tx.sign(&priv_key, 1).map_err(|e| ServiceError::IncentiveError(format!("Failed to sign tx: {}", e)))?;
        Ok(tx)
    }

    async fn broadcast_tx(&self, tx: &SvTx) -> Result<(), ServiceError> {
        let mut stream = TcpStream::connect(&self.transaction_service_addr)
            .await
            .map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let request = BroadcastTxRequest {
            tx: tx.clone(),
            token: self.auth_token.clone(),
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: BroadcastTxResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;

        if response.success {
            Ok(())
        } else {
            Err(ServiceError::IncentiveError(response.error))
        }
    }

    async fn calculate_bonus(&self, block_hash: &str, start: Instant) -> u64 {
        let mut bonus = 0;

        // Speed bonus: 10 sat if response <500ms
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms < 500.0 {
            bonus += self.config.proof_bonus_speed; // 10 sat
            info!("Speed bonus of {} sat applied (response time: {}ms)", self.config.proof_bonus_speed, elapsed_ms);
        }

        // Rarity bonus: 50 sat if <3 seeders
        let seeders = self.tracker.get_seeders(block_hash).await.unwrap_or_default();
        if seeders.len() < 3 {
            bonus += self.config.proof_bonus_rare; // 50 sat
            info!("Rarity bonus of {} sat applied ({} seeders)", self.config.proof_bonus_rare, seeders.len());
        }

        bonus
    }
}
