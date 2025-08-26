use std::env;
use std::num::NonZeroU32;
use std::sync::Arc;

use bincode::{deserialize, serialize};
use chrono::{Duration, Utc};
use dotenv::dotenv;
use governor::{Quota, RateLimiter};
use hex;
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::{AgedThreshold, ShardManager};
use sv::block::Block;
use sv::util::{deserialize as sv_deserialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
struct ValidateBlockRequest {
    block_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateBlockResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsRequest {}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsResponse {
    service_name: String,
    requests_total: u64,
    avg_latency_ms: f64,
    errors_total: u64,
    cache_hits: u64,
    alert_count: u64,
    index_throughput: f64,
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
enum BlockRequestType {
    ValidateBlock { request: ValidateBlockRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
    GetAgedBlocks(GetAgedBlocksRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum BlockResponseType {
    ValidateBlock(ValidateBlockResponse),
    GetMetrics(GetMetricsResponse),
    GetAgedBlocks(GetAgedBlocksResponse),
}

#[derive(Debug)]
struct BlockService {
    auth_service_addr: String,
    alert_service_addr: String,
    storage_service_addr: String,
    consensus_service_addr: String,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<gov::state::NotKeyed, gov::state::InMemoryState, gov::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByTimestampRequest {
    before_timestamp: i64, // Unix timestamp in seconds
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
    GetBlocksByTimestamp { request: GetBlocksByTimestampRequest, token: String },
    GetBlocksByHeight { request: GetBlocksByHeightRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    GetBlocksByTimestamp(GetBlocksByTimestampResponse),
    GetBlocksByHeight(GetBlocksByHeightResponse),
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateBlockConsensusRequest {
    block_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateBlockConsensusResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum ConsensusRequestType {
    ValidateBlock { request: ValidateBlockConsensusRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum ConsensusResponseType {
    ValidateBlock(ValidateBlockConsensusResponse),
}

impl BlockService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("block_requests_total", "Total block requests").unwrap();
        let latency_ms = Gauge::new("block_latency_ms", "Average block request latency").unwrap();
        let alert_count = Counter::new("block_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("block_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        BlockService {
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            storage_service_addr: env::var("STORAGE_ADDR").unwrap_or("127.0.0.1:50053".to_string()),
            consensus_service_addr: env::var("CONSENSUS_ADDR").unwrap_or("127.0.0.1:50055".to_string()),
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
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: AuthResponse = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
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
            service: "block_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: AuthorizeResponse = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
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
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: AlertResponse = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        if response.success {
            self.alert_count.inc();
            Ok(())
        } else {
            warn!("Alert sending failed: {}", response.error);
            Err(response.error)
        }
    }

    async fn fetch_blocks_by_timestamp(&self, before_timestamp: i64, token: &str) -> Result<Vec<Block>, String> {
        let mut stream = TcpStream::connect(&self.storage_service_addr).await
            .map_err(|e| format!("Failed to connect to storage_service: {}", e))?;
        let request = StorageRequestType::GetBlocksByTimestamp { request: GetBlocksByTimestampRequest {
            before_timestamp,
        }, token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: StorageResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        match response {
            StorageResponseType::GetBlocksByTimestamp(resp) => {
                if resp.error.is_empty() {
                    Ok(resp.blocks)
                } else {
                    Err(resp.error)
                }
            }
            _ => Err("Unexpected response type".to_string()),
        }
    }

    async fn fetch_blocks_by_height(&self, max_height: u64, token: &str) -> Result<Vec<Block>, String> {
        let mut stream = TcpStream::connect(&self.storage_service_addr).await
            .map_err(|e| format!("Failed to connect to storage_service: {}", e))?;
        let request = StorageRequestType::GetBlocksByHeight { request: GetBlocksByHeightRequest {
            max_height,
        }, token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
        let response: StorageResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        match response {
            StorageResponseType::GetBlocksByHeight(resp) => {
                if resp.error.is_empty() {
                    Ok(resp.blocks)
                } else {
                    Err(resp.error)
                }
            }
            _ => Err("Unexpected response type".to_string()),
        }
    }

    async fn get_current_tip_height(&self, token: &str) -> Result<u64, String> {
        let blocks = self.fetch_blocks_by_height(u64::MAX, token).await?;
        if blocks.is_empty() {
            Err("No blocks found".to_string())
        } else {
            // Assume blocks are sorted by height; return height of last. Since height not in Block, placeholder
            Ok(800000)
        }
    }

    async fn validate_block_consensus(&self, block_hex: &str, token: &str) -> Result<ValidateBlockConsensusResponse, String> {
        let mut stream = TcpStream::connect(&self.consensus_service_addr).await
            .map_err(|e| format!("Failed to connect to consensus_service: {}", e))?;
        let request = ConsensusRequestType::ValidateBlock { request: ValidateBlockConsensusRequest { block_hex: block_hex.to_string() }, token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
        let response: ConsensusResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;

        if let ConsensusResponseType::ValidateBlock(resp) = response {
            Ok(resp)
        } else {
            Err("Unexpected response type".to_string())
        }
    }

    async fn handle_request(&self, request: BlockRequestType) -> Result<BlockResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            BlockRequestType::ValidateBlock { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "ValidateBlock").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let block_bytes = hex::decode(&request.block_hex).map_err(|e| {
                    warn!("Invalid block hex: {}", e);
                    let _ = self.send_alert("validate_invalid_block", &format!("Invalid block hex: {}", e), 2);
                    format!("Invalid block hex: {}", e)
                })?;
                let block: Block = sv_deserialize(&block_bytes).map_err(|e| {
                    warn!("Invalid block: {}", e);
                    let _ = self.send_alert("validate_invalid_block_deserialization", &format!("Invalid block: {}", e), 2);
                    format!("Invalid block: {}", e)
                })?;

                let resp = self.validate_block_consensus(&request.block_hex, &token).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(BlockResponseType::ValidateBlock(ValidateBlockResponse {
                    success: resp.success,
                    error: resp.error,
                }))
            }
            BlockRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(BlockResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "block_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                }))
            }
            BlockRequestType::GetAgedBlocks(get_aged_request) => {
                let user_id = self.authenticate(&get_aged_request.token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetAgedBlocks").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let blocks = match get_aged_request.threshold {
                    AgedThreshold::Months(months) => {
                        let cutoff = Utc::now() - Duration::days(months as i64 * 30);
                        self.fetch_blocks_by_timestamp(cutoff.timestamp(), &get_aged_request.token).await
                            .map_err(|e| format!("Failed to fetch aged blocks: {}", e))?
                    }
                    AgedThreshold::Blocks(height) => {
                        let current_tip = self.get_current_tip_height(&get_aged_request.token).await
                            .map_err(|e| format!("Failed to get chain tip: {}", e))?;
                        if height >= current_tip {
                            vec![]
                        } else {
                            self.fetch_blocks_by_height(current_tip - height, &get_aged_request.token).await
                                .map_err(|e| format!("Failed to fetch aged blocks: {}", e))?
                        }
                    }
                };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(BlockResponseType::GetAgedBlocks(GetAgedBlocksResponse {
                    blocks,
                    error: String::new(),
                }))
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Block service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: BlockRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = BlockResponseType::ValidateBlock(ValidateBlockResponse {
                                            success: false,
                                            error: e,
                                        });
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

    let addr = env::var("BLOCK_ADDR").unwrap_or("127.0.0.1:50054".to_string());
    let block_service = BlockService::new().await;
    block_service.run(&addr).await?;
    Ok(())
}
