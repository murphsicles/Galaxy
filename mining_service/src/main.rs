use std::env;
use std::io::Cursor;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;

use async_stream::try_stream;
use bincode::{deserialize, serialize};
use dotenv::dotenv;
use futures::Stream;
use governor::{Quota, RateLimiter};
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sv::messages::Tx;
use sv::util::Serializable;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use toml;
use tracing::{error, info, warn};
use async_channel::Receiver;

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
struct GenerateBlockTemplateRequest {
    tx_hexes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct GenerateBlockTemplateResponse {
    success: bool,
    block_hex: String,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct SubmitMinedBlockRequest {
    block_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct SubmitMinedBlockResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamMiningWorkRequest {
    tx_hexes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamMiningWorkResponse {
    success: bool,
    block_hex: String,
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
struct AssembleBlockRequest {
    tx_hexes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct AssembleBlockResponse {
    success: bool,
    block_hex: String,
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
enum BlockRequestType {
    AssembleBlock { request: AssembleBlockRequest, token: String },
    ValidateBlock { request: ValidateBlockRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum BlockResponseType {
    AssembleBlock(AssembleBlockResponse),
    ValidateBlock(ValidateBlockResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum MiningRequestType {
    GenerateBlockTemplate { request: GenerateBlockTemplateRequest, token: String },
    SubmitMinedBlock { request: SubmitMinedBlockRequest, token: String },
    StreamMiningWork { request: StreamMiningWorkRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum MiningResponseType {
    GenerateBlockTemplate(GenerateBlockTemplateResponse),
    SubmitMinedBlock(SubmitMinedBlockResponse),
    StreamMiningWork(StreamMiningWorkResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct MiningService {
    block_service_addr: String,
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

impl MiningService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("mining_requests_total", "Total mining requests").unwrap();
        let latency_ms = Gauge::new("mining_latency_ms", "Average mining request latency").unwrap();
        let alert_count = Counter::new("mining_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("mining_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        MiningService {
            block_service_addr: env::var("BLOCK_ADDR").unwrap_or("127.0.0.1:50054".to_string()),
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
            service: "mining_service".to_string(),
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

    async fn handle_request(&self, request: MiningRequestType) -> Result<MiningResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            MiningRequestType::GenerateBlockTemplate { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GenerateBlockTemplate").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut stream = TcpStream::connect(&self.block_service_addr).await
                    .map_err(|e| format!("Failed to connect to block_service: {}", e))?;
                let block_request = BlockRequestType::AssembleBlock { request: AssembleBlockRequest { tx_hexes: request.tx_hexes }, token: token.to_string() };
                let encoded = serialize(&block_request).map_err(|e| format!("Serialization error: {}", e))?;
                stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
                let block_response: BlockResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format!("Deserialization error: {}", e))?;
                
                let block_response = match block_response {
                    BlockResponseType::AssembleBlock(response) => response,
                    _ => {
                        warn!("Unexpected response from block_service");
                        let _ = self.send_alert("generate_block_template_failed", "Unexpected response from block_service", 2);
                        return Ok(MiningResponseType::GenerateBlockTemplate(GenerateBlockTemplateResponse {
                            success: false,
                            block_hex: "".to_string(),
                            error: "Unexpected response from block_service".to_string(),
                        }));
                    }
                };

                if !block_response.success {
                    warn!("Block assembly failed: {}", block_response.error);
                    let _ = self.send_alert("generate_block_template_failed", &format!("Block assembly failed: {}", block_response.error), 2);
                    return Ok(MiningResponseType::GenerateBlockTemplate(GenerateBlockTemplateResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: block_response.error,
                    }));
                }

                if block_response.block_hex.len() > 34_359_738_368 {
                    warn!("Block size exceeds 32GB: {}", block_response.block_hex.len());
                    let _ = self.send_alert("generate_block_template_size_exceeded", &format!("Block size exceeds 32GB: {}", block_response.block_hex.len()), 3);
                    return Ok(MiningResponseType::GenerateBlockTemplate(GenerateBlockTemplateResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: "Block size exceeds 32GB".to_string(),
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(MiningResponseType::GenerateBlockTemplate(GenerateBlockTemplateResponse {
                    success: true,
                    block_hex: block_response.block_hex,
                    error: "".to_string(),
                }))
            }
            MiningRequestType::SubmitMinedBlock { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "SubmitMinedBlock").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let block_bytes = hex::decode(&request.block_hex).map_err(|e| {
                    warn!("Invalid block hex: {}", e);
                    let _ = self.send_alert("submit_mined_block_invalid_hex", &format!("Invalid block hex: {}", e), 2);
                    format!("Invalid block hex: {}", e)
                })?;
                let block: Block = Serializable::read(&mut Cursor::new(&block_bytes)).map_err(|e| {
                    warn!("Invalid block: {}", e);
                    let _ = self.send_alert("submit_mined_block_invalid_deserialization", &format!("Invalid block: {}", e), 2);
                    format!("Invalid block: {}", e)
                })?;

                if block.size() > 34_359_738_368 {
                    warn!("Block size exceeds 32GB: {}", block.size());
                    let _ = self.send_alert("submit_mined_block_size_exceeded", &format("Block size exceeds 32GB: {}", block.size()), 3);
                    return Ok(MiningResponseType::SubmitMinedBlock(SubmitMinedBlockResponse {
                        success: false,
                        error: "Block size exceeds 32GB".to_string(),
                    }));
                }

                let mut stream = TcpStream::connect(&self.block_service_addr).await
                    .map_err(|e| format!("Failed to connect to block_service: {}", e))?;
                let block_request = BlockRequestType::ValidateBlock { request: ValidateBlockRequest { block_hex: request.block_hex.clone() }, token: token.to_string() };
                let encoded = serialize(&block_request).map_err(|e| format!("Serialization error: {}", e))?;
                stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
                stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
                let block_response: BlockResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format!("Deserialization error: {}", e))?;
                
                let block_response = match block_response {
                    BlockResponseType::ValidateBlock(response) => response,
                    _ => {
                        warn!("Unexpected response from block_service");
                        let _ = self.send_alert("submit_mined_block_validation_failed", "Unexpected response from block_service", 2);
                        return Ok(MiningResponseType::SubmitMinedBlock(SubmitMinedBlockResponse {
                            success: false,
                            error: "Unexpected response from block_service".to_string(),
                        }));
                    }
                };

                if !block_response.success {
                    warn!("Block validation failed: {}", block_response.error);
                    let _ = self.send_alert("submit_mined_block_failed", &format!("Block validation failed: {}", block_response.error), 2);
                    return Ok(MiningResponseType::SubmitMinedBlock(SubmitMinedBlockResponse {
                        success: false,
                        error: block_response.error,
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(MiningResponseType::SubmitMinedBlock(SubmitMinedBlockResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            MiningRequestType::StreamMiningWork { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "StreamMiningWork").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut stream = TcpStream::connect(&self.block_service_addr).await
                    .map_err(|e| format!("Failed to connect to block_service: {}", e))?;
                let block_request = BlockRequestType::AssembleBlock { request: AssembleBlockRequest { tx_hexes: request.tx_hexes }, token: token.to_string() };
                let encoded = serialize(&block_request).map_err(|e| format!("Serialization error: {}", e))?;
                stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
                stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
                let block_response: BlockResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format!("Deserialization error: {}", e))?;
                
                let block_response = match block_response {
                    BlockResponseType::AssembleBlock(response) => response,
                    _ => {
                        warn!("Unexpected response from block_service");
                        let _ = self.send_alert("stream_mining_work_failed", "Unexpected response from block_service", 2);
                        return Ok(MiningResponseType::StreamMiningWork(StreamMiningWorkResponse {
                            success: false,
                            block_hex: "".to_string(),
                            error: "Unexpected response from block_service".to_string(),
                        }));
                    }
                };

                if !block_response.success {
                    warn!("Block assembly failed: {}", block_response.error);
                    let _ = self.send_alert("stream_mining_work_failed", &format!("Block assembly failed: {}", block_response.error), 2);
                    return Ok(MiningResponseType::StreamMiningWork(StreamMiningWorkResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: block_response.error,
                    }));
                }

                if block_response.block_hex.len() > 34_359_738_368 {
                    warn!("Block size exceeds 32GB: {}", block_response.block_hex.len());
                    let _ = self.send_alert("stream_mining_work_size_exceeded", &format!("Block size exceeds 32GB: {}", block_response.block_hex.len()), 3);
                    return Ok(MiningResponseType::StreamMiningWork(StreamMiningWorkResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: "Block size exceeds 32GB".to_string(),
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(MiningResponseType::StreamMiningWork(StreamMiningWorkResponse {
                    success: true,
                    block_hex: block_response.block_hex,
                    error: "".to_string(),
                }))
            }
            MiningRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(MiningResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "mining_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                }))
            }
        }
    }

    async fn stream_mining_work(&self, requests: Receiver<StreamMiningWorkRequest>, token: String) -> impl Stream<Item = Result<StreamMiningWorkResponse, String>> {
        let service = self;
        try_stream! {
            let user_id = service.authenticate(&token).await
                .map_err(|e| format("Authentication failed: {}", e))?;
            service.authorize(&user_id, "StreamMiningWork").await
                .map_err(|e| format("Authorization failed: {}", e))?;
            service.rate_limiter.until_ready().await;

            while let Ok(req) = requests.recv().await {
                service.requests_total.inc();
                let start = std::time::Instant::now();

                let mut stream = TcpStream::connect(&service.block_service_addr).await
                    .map_err(|e| format("Failed to connect to block_service: {}", e))?;
                let block_request = BlockRequestType::AssembleBlock { request: AssembleBlockRequest { tx_hexes: req.tx_hexes }, token: token.to_string() };
                let encoded = serialize(&block_request).map_err(|e| format("Serialization error: {}", e))?;
                stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
                let block_response: BlockResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format("Deserialization error: {}", e))?;
                
                let block_response = match block_response {
                    BlockResponseType::AssembleBlock(response) => response,
                    _ => {
                        warn!("Unexpected response from block_service");
                        let _ = service.send_alert("stream_mining_work_failed", "Unexpected response from block_service", 2);
                        yield StreamMiningWorkResponse {
                            success: false,
                            block_hex: "".to_string(),
                            error: "Unexpected response from block_service".to_string(),
                        };
                        continue;
                    }
                };

                if !block_response.success {
                    warn!("Block assembly failed: {}", block_response.error);
                    let _ = service.send_alert("stream_mining_work_failed", &format("Block assembly failed: {}", block_response.error), 2);
                    yield StreamMiningWorkResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: block_response.error,
                    };
                    continue;
                }

                if block_response.block_hex.len() > 34_359_738_368 {
                    warn!("Block size exceeds 32GB: {}", block_response.block_hex.len());
                    let _ = service.send_alert("stream_mining_work_size_exceeded", &format("Block size exceeds 32GB: {}", block_response.block_hex.len()), 3);
                    yield StreamMiningWorkResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: "Block size exceeds 32GB".to_string(),
                    };
                    continue;
                }

                yield StreamMiningWorkResponse {
                    success: true,
                    block_hex: block_response.block_hex,
                    error: "".to_string(),
                };
                service.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Mining service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: MiningRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = MiningResponseType::GenerateBlockTemplate(GenerateBlockTemplateResponse {
                                            success: false,
                                            block_hex: "".to_string(),
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

    let addr = env::var("MINING_ADDR").unwrap_or("127.0.0.1:50058".to_string());
    let mining_service = MiningService::new().await;
    mining_service.run(&addr).await?;
    Ok(())
}
