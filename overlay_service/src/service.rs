// overlay_service/src/service.rs
use bincode::{deserialize, serialize};
use sled::{Db, IVec};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, error};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct StoreTorrentRefRequest {
    info_hash: String,
    block_hashes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct StoreTorrentRefResponse {
    success: bool,
    error: String,
}

pub struct OverlayService {
    db: Arc<Mutex<Db>>,
}

impl OverlayService {
    pub fn new(db_path: &str) -> Self {
        let db = sled::open(db_path).expect("Failed to open sled db for overlay");
        Self { db: Arc::new(Mutex::new(db)) }
    }

    pub async fn store_torrent_ref(&self, info_hash: &str, block_hashes: Vec<String>) -> Result<(), String> {
        let db = self.db.lock().await;
        let key = info_hash.as_bytes();
        let value = serialize(&block_hashes).map_err(|e| format!("Serialization error: {}", e))?;
        db.insert(key, value).map_err(|e| format!("Sled insert error: {}", e))?;
        db.flush().map_err(|e| format!("Sled flush error: {}", e))?;
        info!("Stored torrent ref for info_hash: {} with {} block hashes", info_hash, block_hashes.len());
        Ok(())
    }

    pub async fn handle_request(&self, request: &[u8]) -> Vec<u8> {
        let req: StoreTorrentRefRequest = match deserialize(request) {
            Ok(req) => req,
            Err(e) => {
                error!("Deserialization error: {}", e);
                let resp = StoreTorrentRefResponse { success: false, error: e.to_string() };
                return serialize(&resp).unwrap();
            }
        };

        match self.store_torrent_ref(&req.info_hash, req.block_hashes).await {
            Ok(_) => {
                let resp = StoreTorrentRefResponse { success: true, error: String::new() };
                serialize(&resp).unwrap()
            }
            Err(e) => {
                error!("Store error: {}", e);
                let resp = StoreTorrentRefResponse { success: false, error: e };
                serialize(&resp).unwrap()
            }
        }
    }

    pub async fn get_block_hashes(&self, info_hash: &str) -> Result<Vec<String>, String> {
        let db = self.db.lock().await;
        let key = info_hash.as_bytes();
        match db.get(key).map_err(|e| format!("Sled get error: {}", e))? {
            Some(value) => {
                let block_hashes: Vec<String> = deserialize(&value).map_err(|e| format!("Deserialization error: {}", e))?;
                Ok(block_hashes)
            }
            None => Err("No entry found for info_hash".to_string()),
        }
    }
}
