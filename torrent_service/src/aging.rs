// torrent_service/src/aging.rs
use crate::utils::{Config, AgedThreshold, ServiceError};
use chrono::{Duration, Utc};
use tokio::time::{sleep, Duration as TokDuration};
use tokio::sync::mpsc::Sender;
use sv::block::Block;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use tracing::{info, error};
use serde::{Deserialize, Serialize};

pub struct AgingManager {
    config: Config,
    event_tx: Sender<super::service::BlockRequestEvent>,
    block_service_addr: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetAgedBlocksRequest {
    threshold: AgedThreshold,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetAgedBlocksResponse {
    blocks: Vec<Block>,
    error: String,
}

impl AgingManager {
    pub fn new(config: &Config, event_tx: Sender<super::service::BlockRequestEvent>) -> Self {
        Self {
            config: config.clone(),
            event_tx,
            block_service_addr: "127.0.0.1:50054".to_string(),
        }
    }

    pub async fn run(&self) {
        info!("Starting aging manager with threshold: {:?}", self.config.aged_threshold);
        loop {
            match self.detect_aged().await {
                Ok(aged_blocks) if !aged_blocks.is_empty() => {
                    info!("Detected {} aged blocks", aged_blocks.len());
                    if let Err(e) = self.event_tx.send(super::service::BlockRequestEvent::AgedBlocks(aged_blocks)).await {
                        error!("Failed to send aged blocks event: {}", e);
                    }
                }
                Ok(_) => info!("No aged blocks detected"),
                Err(e) => error!("Error detecting aged blocks: {}", e),
            }
            sleep(TokDuration::from_secs(3600)).await; // Check hourly
        }
    }

    async fn detect_aged(&self) -> Result<Vec<Block>, ServiceError> {
        let mut stream = TcpStream::connect(&self.block_service_addr)
            .await
            .map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let request = GetAgedBlocksRequest {
            threshold: self.config.aged_threshold.clone(),
            token: "dummy_token".to_string(), // Replace with proper auth
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: GetAgedBlocksResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;

        if response.error.is_empty() {
            Ok(response.blocks)
        } else {
            Err(ServiceError::Torrent(response.error))
        }
    }
}
