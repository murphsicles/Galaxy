// torrent_service/src/incentives.rs
use crate::utils::{Config, ServiceError};
use bsv::{Transaction as BsvTx, PrivateKey, PublicKey, Script, Opcode};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use tracing::{info, error};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct IncentivesManager {
    config: Config,
    transaction_service_addr: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxRequest {
    tx: BsvTx,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxResponse {
    success: bool,
    error: String,
}

impl IncentivesManager {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            transaction_service_addr: "127.0.0.1:50053".to_string(), // Assume port for transaction_service
        }
    }

    pub async fn reward_proof(&self, user_id: &str) -> Result<(), ServiceError> {
        let reward = self.config.proof_reward_base; // 100 sat base
        let bonus = self.calculate_bonus().await; // Placeholder for speed/rarity
        let total_reward = reward + bonus;

        let tx = self.build_op_return_tx(user_id, total_reward, "proof_reward").await?;
        self.broadcast_tx(&tx).await?;
        info!("Proof reward of {} sat sent to user_id: {}", total_reward, user_id);
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

    async fn build_op_return_tx(&self, user_id: &str, amount: u64, action: &str) -> Result<BsvTx, ServiceError> {
        // Placeholder: Generate a private key for signing (in production, use proper wallet key)
        let priv_key = PrivateKey::from_random();
        let pub_key = PublicKey::from_private_key(&priv_key);

        // Create OP_RETURN script with action and user_id
        let mut script = Script::new();
        script.append(Opcode::OP_RETURN);
        script.append_data(action.as_bytes());
        script.append_data(user_id.as_bytes());

        // Build transaction
        let mut tx = BsvTx::new();
        tx.add_output(amount, &script);
        // Placeholder: Add input from wallet (requires actual utxos)
        // tx.add_input(...);

        // Sign transaction (simplified; real impl needs utxo details)
        tx.sign(&priv_key, 1 /* SIGHASH_ALL */).map_err(|e| ServiceError::IncentiveError(format!("Failed to sign tx: {}", e)))?;
        Ok(tx)
    }

    async fn broadcast_tx(&self, tx: &BsvTx) -> Result<(), ServiceError> {
        let mut stream = TcpStream::connect(&self.transaction_service_addr)
            .await
            .map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let request = BroadcastTxRequest {
            tx: tx.clone(),
            token: "dummy_token".to_string(), // Replace with proper auth
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

    async fn calculate_bonus(&self) -> u64 {
        // Placeholder: Implement speed (<500ms) and rarity (seeder count) logic
        // For now, return fixed bonus for testing
        10 // 10 sat bonus
    }
}
