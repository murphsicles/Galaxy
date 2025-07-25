// torrent_service/src/proof_server.rs
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use crate::utils::{Config, ServiceError};
use crate::tracker::TrackerManager;
use sv::merkle::MerklePath;
use sv::transaction::Transaction as SvTx;
use sv::block::Header;
use tokio::sync::mpsc::Sender;
use tracing::{info, error};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub struct ProofServer {
    config: Config,
    tracker: Arc<TrackerManager>,
    event_tx: Sender<super::service::BlockRequestEvent>,
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
        Self {
            config: config.clone(),
            tracker: Arc::new(TrackerManager::new(config).await),
            event_tx,
        }
    }

    pub async fn get_proof(&self, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        let peers = self.tracker.get_seeders(block_hash).await?;
        if peers.is_empty() {
            return Err(ServiceError::Torrent("No seeders available for block".to_string()));
        }

        for peer in peers {
            match self.request_proof_from_peer(&peer, txid, block_hash).await {
                Ok(proof) => return Ok(proof),
                Err(e) => {
                    error!("Failed to get proof from peer {}: {}", peer, e);
                    continue;
                }
            }
        }

        Err(ServiceError::Torrent("Failed to retrieve proof from any seeder".to_string()))
    }

    async fn request_proof_from_peer(&self, peer: &str, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        let mut stream = TcpStream::connect(peer).await.map_err(|e| ServiceError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
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

    pub async fn run(&self) {
        let listener = TcpListener::bind("0.0.0.0:50063").await.unwrap();  // Updated port
        loop {
            if let Ok((mut stream, addr)) = listener.accept().await {
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
        }
    }

    async fn compute_proof(&self, txid: &str, block_hash: &str) -> Result<ProofBundle, ServiceError> {
        let block = Block::default(); // Replace with actual block fetch from local torrent storage
        let tx = block.transactions.iter().find(|t| t.txid().to_string() == txid)
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
