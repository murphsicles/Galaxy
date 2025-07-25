// storage_service/src/service.rs
use bincode::{deserialize, serialize};
use sled::{Db, IVec};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, error};
use serde::{Deserialize, Serialize};
use sv::block::Block;
use std::collections::BTreeMap;
use chrono::Utc;

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByTimestampRequest {
    before_timestamp: i64,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByTimestampResponse {
    blocks: Vec<Block>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByHeightRequest {
    max_height: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByHeightResponse {
    blocks: Vec<Block>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageRequestType {
    GetBlocksByTimestamp(GetBlocksByTimestampRequest),
    GetBlocksByHeight(GetBlocksByHeightRequest),
    // Add more as needed, e.g., StoreBlock
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    GetBlocksByTimestamp(GetBlocksByTimestampResponse),
    GetBlocksByHeight(GetBlocksByHeightResponse),
}

pub struct StorageService {
    db: Arc<Mutex<Db>>,
}

impl StorageService {
    pub fn new(db_path: &str) -> Self {
        let db = sled::open(db_path).expect("Failed to open sled db for storage");
        Self { db: Arc::new(Mutex::new(db)) }
    }

    pub async fn store_block(&self, block: &Block) -> Result<(), String> {
        let db = self.db.lock().await;
        let key_height = format!("height:{}", block.height).as_bytes();
        let key_timestamp = format!("timestamp:{}", block.header.timestamp).as_bytes();
        let serialized = serialize(block).map_err(|e| format!("Serialization error: {}", e))?;

        db.insert(key_height, serialized.clone()).map_err(|e| format!("Sled insert error: {}", e))?;
        db.insert(key_timestamp, serialized).map_err(|e| format!("Sled insert error: {}", e))?;
        db.flush().map_err(|e| format!("Sled flush error: {}", e))?;
        info!("Stored block at height {} with timestamp {}", block.height, block.header.timestamp);
        Ok(())
    }

    pub async fn get_blocks_by_timestamp(&self, before_timestamp: i64) -> Result<Vec<Block>, String> {
        let db = self.db.lock().await;
        let mut blocks = vec![];

        for res in db.range(..) {
            let (key, value) = res.map_err(|e| format!("Sled range error: {}", e))?;
            let key_str = String::from_utf8_lossy(&key);
            if key_str.starts_with("timestamp:") {
                let timestamp = key_str.strip_prefix("timestamp:").unwrap().parse::<i64>().map_err(|e| format!("Parse error: {}", e))?;
                if timestamp < before_timestamp {
                    let block: Block = deserialize(&value).map_err(|e| format!("Deserialization error: {}", e))?;
                    blocks.push(block);
                }
            }
        }

        Ok(blocks)
    }

    pub async fn get_blocks_by_height(&self, max_height: u64) -> Result<Vec<Block>, String> {
        let db = self.db.lock().await;
        let mut blocks = vec![];

        for res in db.range(..) {
            let (key, value) = res.map_err(|e| format!("Sled range error: {}", e))?;
            let key_str = String::from_utf8_lossy(&key);
            if key_str.starts_with("height:") {
                let height = key_str.strip_prefix("height:").unwrap().parse::<u64>().map_err(|e| format!("Parse error: {}", e))?;
                if height < max_height {
                    let block: Block = deserialize(&value).map_err(|e| format!("Deserialization error: {}", e))?;
                    blocks.push(block);
                }
            }
        }

        Ok(blocks)
    }

    pub async fn handle_request(&self, request: StorageRequestType) -> StorageResponseType {
        match request {
            StorageRequestType::GetBlocksByTimestamp(req) => {
                match self.get_blocks_by_timestamp(req.before_timestamp).await {
                    Ok(blocks) => StorageResponseType::GetBlocksByTimestamp(GetBlocksByTimestampResponse {
                        blocks,
                        error: String::new(),
                    }),
                    Err(e) => StorageResponseType::GetBlocksByTimestamp(GetBlocksByTimestampResponse {
                        blocks: vec![],
                        error: e,
                    }),
                }
            }
            StorageRequestType::GetBlocksByHeight(req) => {
                match self.get_blocks_by_height(req.max_height).await {
                    Ok(blocks) => StorageResponseType::GetBlocksByHeight(GetBlocksByHeightResponse {
                        blocks,
                        error: String::new(),
                    }),
                    Err(e) => StorageResponseType::GetBlocksByHeight(GetBlocksByHeightResponse {
                        blocks: vec![],
                        error: e,
                    }),
                }
            }
        }
    }
}
