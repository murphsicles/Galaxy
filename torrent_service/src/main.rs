use std::env;
use std::num::NonZeroU32;
use std::sync::Arc;

use bincode::{deserialize, serialize};
use dotenv::dotenv;
use governor::{Quota, RateLimiter};
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use toml;
use tracing::{error, info, warn};

#[derive(Serialize, Deserialize, Debug)]
struct AuthRequest {
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthResponse {
    success: bool,
    user_id: String,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthorizeRequest {
    user_id: String,
    service: String,
    method: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthorizeResponse {
    allowed: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AlertRequest {
    event_type: String,
    message: String,
    severity: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct AlertResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum TorrentRequestType {
    OffloadAgedBlocks { token: String },
    GetProof { txid: String, block_hash: String, token: String },
    GetMetrics { token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum TorrentResponseType {
    OffloadSuccess { success: bool, error: String },
    ProofBundle { proof: String, error: String },
    Metrics { metrics: GetMetricsResponse },
}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsResponse {
    service_name: String,
    requests_total: u64,
    avg_latency_ms: f64,
    errors_total: u64,
    alert_count: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetAgedBlocksRequest {
    threshold: AgedThreshold,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetAgedBlocksResponse {
    blocks: Vec<Block>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum BlockRequestType {
    GetAgedBlocks(GetAgedBlocksRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum BlockResponseType {
    GetAgedBlocks(GetAgedBlocksResponse),
}

#[derive(Serialize, Deserialize, Debug)]
struct GenerateSPVProofRequest {
    txid: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GenerateSPVProofResponse {
    success: bool,
    merkle_path: String,
    block_headers: Vec<String>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationRequestType {
    GenerateSPVProof { request: GenerateSPVProofRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationResponseType {
    GenerateSPVProof(GenerateSPVProofResponse),
}

#[derive(Debug)]
struct TorrentService {
    block_service_addr: String,
    validation_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<gov::state::NotKeyed, gov::state::InMemoryState, gov::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl TorrentService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("torrent_requests_total", "Total torrent requests").unwrap();
        let latency_ms = Gauge::new("torrent_latency_ms", "Average torrent request latency").unwrap();
        let alert_count = Counter::new("torrent_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("torrent_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        TorrentService {
            block_service_addr: env::var("BLOCK_ADDR").unwrap_or("127.0.0.1:50054".to_string()),
            validation_service_addr: env::var("VALIDATION_ADDR").unwrap_or("127.0.0.1:50057".to_string()),
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            registry,
            requests_total,
            latency_ms,
            alert_count,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
        }
    }

    async fn authenticate(&self, token: &str) -> Result<String, String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await
            .map_err(|e| format!("Failed to connect to auth_service: {}", e))?;
        let request = AuthRequest { token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format("Read error: {}", e))?;
        let response: AuthResponse = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
        if response.success {
            Ok(response.user_id)
        } else {
            Err(response.error)
        }
    }

    async fn authorize(&self, user_id: &str, method: &str) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await
            .map_err(|e| format!("Failed to connect to auth_service: {}", e))?;
        let request = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "torrent_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format("Write error: {}", e))?;
        stream.flush().await map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format("Read error: {}", e))?;
        let response: AuthorizeResponse = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
        if response.allowed {
            Ok(())
        } else {
            Err(response.error)
        }
    }

    async fn send_alert(&self, event_type: &str, message: &str, severity: u32) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.alert_service_addr).await
            .map_err(|e| format!("Failed to connect to alert_service: {}", e))?;
        let request = AlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format("Write error: {}", e))?;
        stream.flush().await map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format("Read error: {}", e))?;
        let response: AlertResponse = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
        if response.success {
            self.alert_count.inc();
            Ok(())
        } else {
            warn!("Alert sending failed: {}", response.error);
            Err(response.error)
        }
    }

    async fn handle_request(&self, request: TorrentRequestType) -> Result<TorrentResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            TorrentRequestType::OffloadAgedBlocks { token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "OffloadAgedBlocks").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                // Fetch aged blocks from block_service
                let mut stream = TcpStream::connect(&self.block_service_addr).await
                    .map_err(|e| format("Failed to connect to block_service: {}", e))?;
                let block_request = BlockRequestType::GetAgedBlocks(GetAgedBlocksRequest { threshold: AgedThreshold::Months(1), token: token.to_string() });
                let encoded = serialize(&block_request).map_err(|e| format("Serialization error: {}", e))?;
                stream.write_all(&encoded).await map_err(|e| format("Write error: {}", e))?;
                stream.flush().await map_err(|e| format("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await map_err(|e| format("Read error: {}", e))?;
                let block_response: BlockResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format("Deserialization error: {}", e))?;

                let blocks = match block_response {
                    BlockResponseType::GetAgedBlocks(response) => response.blocks,
                    _ => {
                        warn!("Unexpected response from block_service");
                        let _ = self.send_alert("offload_aged_blocks_failed", "Unexpected response from block_service", 2);
                        return Err("Unexpected response from block_service".to_string());
                    }
                };

                // Dummy torrent creation (in real, use rust-libtorrent or similar to create torrent file from blocks data)
                for block in blocks {
                    let serialized = serialize(&block).map_err(|e| format("Serialization error: {}", e))?;
                    // Create torrent file from serialized block (placeholder)
                    info!("Offloaded block to torrent: {}", hex::encode(block.header.hash().0));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TorrentResponseType::OffloadSuccess { success: true, error: "".to_string() })
            }
            TorrentRequestType::GetProof { txid, block_hash, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetProof").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                // Fetch SPV proof from validation_service
                let mut stream = TcpStream::connect(&self.validation_service_addr).await
                    .map_err(|e| format("Failed to connect to validation_service: {}", e))?;
                let validation_request = ValidationRequestType::GenerateSPVProof { request: GenerateSPVProofRequest { txid }, token: token.to_string() };
                let encoded = serialize(&validation_request).map_err(|e| format("Serialization error: {}", e))?;
                stream.write_all(&encoded).await map_err(|e| format("Write error: {}", e))?;
                stream.flush().await map_err(|e| format("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await map_err(|e| format("Read error: {}", e))?;
                let validation_response: ValidationResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format("Deserialization error: {}", e))?;

                let proof = match validation_response {
                    ValidationResponseType::GenerateSPVProof(response) => {
                        if response.success {
                            format!("Merkle Path: {}, Headers: {:?}", response.merkle_path, response.block_headers)
                        } else {
                            warn!("Proof generation failed: {}", response.error);
                            let _ = self.send_alert("get_proof_failed", &format("Proof generation failed: {}", response.error), 2);
                            return Err(response.error);
                        }
                    }
                    _ => {
                        warn!("Unexpected response from validation_service");
                        let _ = self.send_alert("get_proof_failed", "Unexpected response from validation_service", 2);
                        return Err("Unexpected response from validation_service".to_string());
                    }
                };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TorrentResponseType::ProofBundle { proof, error: "".to_string() })
            }
            TorrentRequestType::GetMetrics { token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TorrentResponseType::Metrics { metrics: GetMetricsResponse {
                    service_name: "torrent_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    alert_count: self.alert_count.get() as u64,
                } })
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Torrent service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: TorrentRequestType = match deserialize(&buffer[..n]) {
                                    Ok(req) => req,
                                    Err(e) => {
                                        error!("Deserialization error: {}", e);
                                        service.errors_total.inc();
                                        return;
                                    }
                                };
                                match service.handle_request(request).await {
                                    Ok(response) => {
                                        let encoded = serialize(&response).unwrap();
                                        if let Err(e) = stream.write_all(&encoded).await {
                                            error!("Write error: {}", e);
                                            service.errors_total.inc();
                                        }
                                        if let Err(e) = stream.flush().await {
                                            error!("Flush error: {}", e);
                                            service.errors_total.inc();
                                        }
                                    }
                                    Err(e) => {
                                        error!("Request error: {}", e);
                                        service.errors_total.inc();
                                        let response = TorrentResponseType::OffloadSuccess {
                                            success: false,
                                            error: e,
                                        };
                                        let encoded = serialize(&response).unwrap();
                                        if let Err(e) = stream.write_all(&encoded).await {
                                            error!("Write error: {}", e);
                                        }
                                        if let Err(e) = stream.flush().await {
                                            error!("Flush error: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Read error: {}", e);
                                service.errors_total.inc();
                            }
                        }
                    });
                }
                Err(e) => error!("Accept error: {}", e),
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = env::var("TORRENT_ADDR").unwrap_or("127.0.0.1:50062".to_string());
    let torrent_service = TorrentService::new().await;
    torrent_service.run(&addr).await?;
    Ok(())
}
