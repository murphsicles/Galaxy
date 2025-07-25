// torrent_service/src/service.rs
use crate::aging::AgingManager;
use crate::chunker::Chunker;
use crate::incentives::IncentivesManager;
use crate::proof_server::{ProofServer, ProofBundle};
use crate::tracker::TrackerManager;
use crate::utils::{Config, ServiceError, AgedThreshold};
use shared::ShardManager;
use sv::block::Block;
use tokio::sync::mpsc;
use tracing::{info, warn, error};
use prometheus::{Counter, Gauge, Registry};
use std::sync::Arc;
use toml;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use bincode::{deserialize, serialize};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use serde::{Deserialize, Serialize};

pub struct TorrentService {
    pub config: Config,
    chunker: Arc<Chunker>,
    tracker: Arc<TrackerManager>,
    proof_server: Arc<ProofServer>,
    incentives: Arc<IncentivesManager>,
    aging: Arc<AgingManager>,
    shard_manager: Arc<ShardManager>,
    auth_service_addr: String,
    alert_service_addr: String,
    block_service_addr: String,
    validation_service_addr: String,
    overlay_service_addr: String,
    pub registry: Arc<Registry>,
    pub requests_total: Counter,
    pub latency_ms: Gauge,
    pub alert_count: Counter,
    pub errors_total: Counter,
    pub rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    event_rx: mpsc::Receiver<BlockRequestEvent>,
    auth_token: String,
}

#[derive(Debug)]
enum BlockRequestEvent {
    AgedBlocks(Vec<Block>),
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

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofRequest {
    proof: ProofBundle,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofResponse {
    success: bool,
    error: String,
}

impl TorrentService {
    pub async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let toml_config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let config = Config {
            piece_size: toml_config.get("torrent_service").and_then(|s| s.get("piece_size").and_then(|v| v.as_integer())).unwrap_or(32 * 1024 * 1024) as usize,
            aged_threshold: AgedThreshold::Months(toml_config.get("torrent_service").and_then(|s| s.get("aged_threshold_months").and_then(|v| v.as_integer())).unwrap_or(60) as u32),
            stake_amount: 100000,
            proof_reward_base: 100,
            bulk_reward_per_mb: 100,
            proof_bonus_speed: 10,
            proof_bonus_rare: 50,
            tracker_port: Some(6969),
            proof_rpc_port: Some(50063),
        };

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("torrent_requests_total", "Total torrent requests").unwrap();
        let latency_ms = Gauge::new("torrent_latency_ms", "Average torrent request latency").unwrap();
        let alert_count = Counter::new("torrent_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("torrent_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));

        let (event_tx, event_rx) = mpsc::channel(32);
        let chunker = Arc::new(Chunker::new(&config));
        let tracker = Arc::new(TrackerManager::new(&config).await);
        let proof_server = Arc::new(ProofServer::new(&config, event_tx.clone()));
        let incentives = Arc::new(IncentivesManager::new(&config));
        let aging = Arc::new(AgingManager::new(&config, event_tx));

        let auth_token = toml_config.get("torrent_service").and_then(|s| s.get("auth_token").and_then(|v| v.as_str())).unwrap_or("default_token").to_string();

        Self {
            config,
            chunker,
            tracker,
            proof_server,
            incentives,
            aging,
            shard_manager: Arc::new(ShardManager::new()),
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
            block_service_addr: "127.0.0.1:50054".to_string(),
            validation_service_addr: "127.0.0.1:50057".to_string(),
            overlay_service_addr: "127.0.0.1:50056".to_string(),
            registry,
            requests_total,
            latency_ms,
            alert_count,
            errors_total,
            rate_limiter,
            event_rx,
            auth_token,
        }
    }

    async fn authenticate(&self, token: &str) -> Result<String, ServiceError> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await.map_err(ServiceError::from)?;
        let request = super::main::AuthRequest { token: token.to_string() };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: super::main::AuthResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;
        
        if response.success {
            Ok(response.user_id)
        } else {
            Err(ServiceError::AuthError(response.error))
        }
    }

    async fn authorize(&self, user_id: &str, method: &str) -> Result<(), ServiceError> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await.map_err(ServiceError::from)?;
        let request = super::main::AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "torrent_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: super::main::AuthorizeResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;
        
        if response.allowed {
            Ok(())
        } else {
            Err(ServiceError::AuthError(response.error))
        }
    }

    async fn send_alert(&self, event_type: &str, message: &str, severity: u32) -> Result<(), ServiceError> {
        let mut stream = TcpStream::connect(&self.alert_service_addr).await.map_err(ServiceError::from)?;
        let request = super::main::AlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: super::main::AlertResponse = deserialize(&buffer[..n]).map_err(ServiceError::from)?;
        
        if response.success {
            self.alert_count.inc();
            Ok(())
        } else {
            warn!("Alert sending failed: {}", response.error);
            Err(ServiceError::AlertError(response.error))
        }
    }

    pub async fn handle_request(&self, request: super::main::TorrentRequestType) -> Result<super::main::TorrentResponseType, ServiceError> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            super::main::TorrentRequestType::OffloadAgedBlocks { token } => {
                let user_id = self.authenticate(&token).await?;
                self.authorize(&user_id, "OffloadAgedBlocks").await?;
                self.rate_limiter.until_ready().await;

                let blocks = self.fetch_aged_blocks().await?;
                let torrent = self.chunker.offload(&blocks).await?;
                self.tracker.announce(&torrent).await?;
                self.store_torrent_ref(&torrent.info_hash()).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(super::main::TorrentResponseType::OffloadSuccess { success: true, error: String::new() })
            }
            super::main::TorrentRequestType::GetProof { txid, block_hash, token } => {
                let user_id = self.authenticate(&token).await?;
                self.authorize(&user_id, "GetProof").await?;
                self.rate_limiter.until_ready().await;

                let proof = self.proof_server.get_proof(&txid, &block_hash).await?;
                self.validate_proof(&proof).await?;
                let serialized_proof = serialize(&proof).map_err(ServiceError::from)?;

                self.incentives.reward_proof(&user_id, &block_hash).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(super::main::TorrentResponseType::ProofBundle { proof: hex::encode(serialized_proof), error: String::new() })
            }
            super::main::TorrentRequestType::GetMetrics { token } => {
                let user_id = self.authenticate(&token).await?;
                self.authorize(&user_id, "GetMetrics").await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(super::main::TorrentResponseType::Metrics { metrics: super::main::GetMetricsResponse {
                    service_name: "torrent_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    alert_count: self.alert_count.get() as u64,
                } })
            }
        }
    }

    async fn fetch_aged_blocks(&self) -> Result<Vec<Block>, ServiceError> {
        let mut stream = TcpStream::connect(&self.block_service_addr).await
            .map_err(ServiceError::from)?;
        let request = GetAgedBlocksRequest {
            threshold: self.config.aged_threshold.clone(),
            token: self.auth_token.clone(),
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: GetAgedBlocksResponse = deserialize(&buffer[..n])
            .map_err(ServiceError::from)?;
        
        if response.error.is_empty() {
            Ok(response.blocks)
        } else {
            Err(ServiceError::Torrent(response.error))
        }
    }

    async fn store_torrent_ref(&self, info_hash: &str) -> Result<(), ServiceError> {
        let mut stream = TcpStream::connect(&self.overlay_service_addr).await
            .map_err(ServiceError::from)?;
        let block_hashes = vec!["placeholder".to_string()];
        let request = StoreTorrentRefRequest {
            info_hash: info_hash.to_string(),
            block_hashes,
        };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: StoreTorrentRefResponse = deserialize(&buffer[..n])
            .map_err(ServiceError::from)?;
        
        if response.success {
            Ok(())
        } else {
            Err(ServiceError::Torrent(response.error))
        }
    }

    async fn validate_proof(&self, proof: &ProofBundle) -> Result<(), ServiceError> {
        let mut stream = TcpStream::connect(&self.validation_service_addr).await
            .map_err(ServiceError::from)?;
        let request = ValidateProofRequest { proof: proof.clone() };
        let encoded = serialize(&request).map_err(ServiceError::from)?;
        stream.write_all(&encoded).await.map_err(ServiceError::from)?;
        stream.flush().await.map_err(ServiceError::from)?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(ServiceError::from)?;
        let response: ValidateProofResponse = deserialize(&buffer[..n])
            .map_err(ServiceError::from)?;
        
        if response.success {
            Ok(())
        } else {
            Err(ServiceError::Torrent(response.error))
        }
    }
}
