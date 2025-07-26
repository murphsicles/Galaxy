// torrent_service/src/proof_server.rs
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use crate::utils::{Config, ServiceError};
use crate::tracker::TrackerManager;
use sv::merkle::MerklePath;
use sv::transaction::Transaction as SvTx;
use sv::block::{Block, Header};
use tokio::sync::mpsc::Sender;
use tracing::{info, error};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use cratetorrent::prelude::*;
use std::path::Path;
use tokio::time::{sleep, Duration};

pub struct ProofServer {
    config: Config,
    tracker: Arc<TrackerManager>,
    event_tx: Sender<super::service::BlockRequestEvent>,
    torrent_client: TorrentClient,
    backup_node_addr: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProofBundle {
    pub tx: SvTx,
    pub path: MerklePath,
    pub header: Header,
}

#[derive(Serialize, Deserialize, Debug)]
struct ProofRequest {
    txid: String,
    block_hash: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ProofResponse {
    proof: Option<ProofBundle>,
    error: String,
}

impl ProofServer {
    pub fn new(config: &Config, event_tx: Sender<super::service::BlockRequestEvent>) -> Self {
        let torrent_config = Config {
            download_path: Path::new("./torrent_data").to_path_buf(),
            ..Config::default()
        };
        let torrent_client = TorrentClient::new(&torrent_config)
            .await
            .expect("Failed to initialize torrent client");
        Self {
            config: config.clone(),
            tracker: Arc::new(TrackerManager::new(config).await),
            event_tx,
            torrent_client,
            backup_node_addr: "127.0.0.1:50064".to_string(), // Backup node address
        }
    }

    pub async fn get_proof(&self, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        // Retry up to 3 times with backoff
        for attempt in 1..=3 {
            let peers = self.tracker.get_seeders(block_hash).await?;
            if !peers.is_empty() {
                for peer in peers {
                    match self.request_proof_from_peer(&peer, txid, block_hash).await {
                        Ok(proof) => return Ok(proof),
                        Err(e) => {
                            error!("Failed to get proof from peer {}: {}", peer, e);
                            continue;
                        }
                    }
                }
            }
            error!("No seeders available for block {} on attempt {}", block_hash, attempt);
            if attempt < 3 {
                sleep(Duration::from_secs(2 * attempt as u64)).await; // Exponential backoff
                continue;
            }

            // Fallback to backup node
            match self.request_proof_from_backup(txid, block_hash).await {
                Ok(proof) => return Ok(proof),
                Err(e) => {
                    error!("Backup node failed for block {}: {}", block_hash, e);
                    // Trigger bounty as last resort
                    self.trigger_bounty(block_hash).await?;
                    return Err(ServiceError::Torrent(format!("No seeders or backup available for block {}; bounty issued", block_hash)));
                }
            }
        }
        Err(ServiceError::Torrent(format!("Failed to retrieve proof for block {} after retries", block_hash)))
    }

    async fn request_proof_from_peer(&self, peer: &str, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        let mut stream = TcpStream::connect(peer).await
            .map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let request = ProofRequest {
            txid: txid.to_string(),
            block_hash: block_hash.to_string(),
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: ProofResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;

        if let Some(proof) = response.proof {
            Ok(proof)
        } else {
            Err(ServiceError::Torrent(response.error))
        }
    }

    async fn request_proof_from_backup(&self, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        let mut stream = TcpStream::connect(&self.backup_node_addr).await
            .map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let request = ProofRequest {
            txid: txid.to_string(),
            block_hash: block_hash.to_string(),
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: ProofResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;

        if let Some(proof) = response.proof {
            Ok(proof)
        } else {
            Err(ServiceError::Torrent(response.error))
        }
    }

    async fn trigger_bounty(&self, block_hash: &str) -> Result<(), ServiceError> {
        // Placeholder: Issue a bounty via incentives.rs
        info!("Issued bounty for block {}", block_hash);
        Ok(())
    }

    pub async fn run(&self) {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.config.proof_rpc_port.unwrap_or(50063)))
            .await
            .expect("Failed to bind proof server");
        info!("Proof server running on port {}", self.config.proof_rpc_port.unwrap_or(50063));

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let this = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: ProofRequest = match deserialize(&buffer[..n]) {
                                    Ok(req) => req,
                                    Err(e) => {
                                        error!("Deserialization error for proof request from {}: {}", addr, e);
                                        let response = ProofResponse {
                                            proof: None,
                                            error: format!("Deserialization error: {}", e),
                                        };
                                        let encoded = serialize(&response).unwrap();
                                        let _ = stream.write_all(&encoded).await;
                                        let _ = stream.flush().await;
                                        return;
                                    }
                                };

                                let proof = match this.compute_proof(&request.txid, &request.block_hash).await {
                                    Ok(proof) => ProofResponse {
                                        proof: Some(proof),
                                        error: String::new(),
                                    },
                                    Err(e) => ProofResponse {
                                        proof: None,
                                        error: e.to_string(),
                                    },
                                };

                                let encoded = serialize(&proof).unwrap();
                                if let Err(e) = stream.write_all(&encoded).await {
                                    error!("Write error to {}: {}", addr, e);
                                }
                                if let Err(e) = stream.flush().await {
                                    error!("Flush error to {}: {}", addr, e);
                                }
                            }
                            Err(e) => {
                                error!("Read error from {}: {}", addr, e);
                            }
                        }
                    });
                }
                Err(e) => error!("Accept error: {}", e),
            }
        }
    }

    async fn compute_proof(&self, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        // Fetch torrent metadata from overlay_service to get info_hash
        let mut stream = TcpStream::connect("127.0.0.1:50056").await
            .map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        let request = super::service::StoreTorrentRefRequest {
            info_hash: block_hash.to_string(),
            block_hashes: vec![block_hash.to_string()],
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: super::service::StoreTorrentRefResponse = deserialize(&buffer[..n])
            .map_err(ServiceError::from)?;
        if !response.success {
            return Err(ServiceError::Torrent(response.error));
        }

        // Load block from local torrent storage
        let torrent = self.torrent_client
            .get_torrent(&block_hash.to_string())
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to load torrent: {}", e)))?;
        let block_data = torrent.read_block(0).await
            .map_err(|e| ServiceError::Torrent(format!("Failed to read block data: {}", e)))?;
        let block: Block = deserialize(&block_data)
            .map_err(|e| ServiceError::Torrent(format!("Deserialization error: {}", e)))?;

        let tx = block.transactions.iter()
            .find(|t| t.txid().to_string() == txid)
            .ok_or_else(|| ServiceError::Torrent(format!("Transaction {} not found in block {}", txid, block_hash)))?;
        let path = MerklePath::from_block(&block, txid)
            .map_err(|e| ServiceError::Torrent(format!("Failed to compute merkle path: {}", e)))?;

        Ok(ProofBundle {
            tx: tx.clone(),
            path,
            header: block.header.clone(),
        })
    }
}
