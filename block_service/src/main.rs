// block_service/src/main.rs
use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use sv::block::Block;
use sv::util::{deserialize as sv_deserialize};
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use toml;
use governor::{Quota, RateLimiter};
use shared::{ShardManager, AgedThreshold};
use chrono::{Duration, Utc};
use hex;

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
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
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
    GetBlocksByTimestamp(GetBlocksByTimestampRequest),
    GetBlocksByHeight(GetBlocksByHeightRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    GetBlocksByTimestamp(GetBlocksByTimestampResponse),
    GetBlocksByHeight(GetBlocksByHeightResponse),
}

impl BlockService {
    async fn new() -> Self {
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
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
            storage_service_addr: "127.0.0.1:50052".to_string(), // Assume port
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
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
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
            service: "block_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
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
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
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

    async fn fetch_blocks_by_timestamp(&self, before_timestamp: i64) -> Result<Vec<Block>, String> {
        let mut stream = TcpStream::connect(&self.storage_service_addr).await
            .map_err(|e| format!("Failed to connect to storage_service: {}", e))?;
        let request = StorageRequestType::GetBlocksByTimestamp(GetBlocksByTimestampRequest {
            before_timestamp,
        });
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
        let response: StorageResponseType = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
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

    async fn fetch_blocks_by_height(&self, max_height: u64) -> Result<Vec<Block>, String> {
        let mut stream = TcpStream::connect(&self.storage_service_addr).await
            .map_err(|e| format!("Failed to connect to storage_service: {}", e))?;
        let request = StorageRequestType::GetBlocksByHeight(GetBlocksByHeightRequest {
            max_height,
        });
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
        let response: StorageResponseType = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
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

    async fn get_current_tip_height(&self) -> Result<u64, String> {
        // Placeholder: Query storage_service or internal state for current chain tip height
        // For now, return a dummy value
        Ok(1000000)
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

                // Placeholder: Validate block against consensus rules
                let success = true;
                let error = if success { "".to_string() } else { "Block validation failed".to_string() };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(BlockResponseType::ValidateBlock(ValidateBlockResponse {
                    success,
                    error,
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
                        self.fetch_blocks_by_timestamp(cutoff.timestamp()).await
                            .map_err(|e| format!("Failed to fetch aged blocks: {}", e))?
                    }
                    AgedThreshold::Blocks(height) => {
                        let current_tip = self.get_current_tip_height().await
                            .map_err(|e| format!("Failed to get chain tip: {}", e))?;
                        if height >= current_tip {
                            vec![]
                        } else {
                            self.fetch_blocks_by_height(current_tip - height).await
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

    let addr = "127.0.0.1:50054";
    let block_service = BlockService::new().await;
    block_service.run(addr).await?;
    Ok(())
}
