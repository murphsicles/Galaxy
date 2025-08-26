use std::env;
use std::num::NonZeroU32;
use std::sync::Arc;

use bincode::{deserialize, serialize};
use dotenv::dotenv;
use governor::{Quota, RateLimiter};
use hex;
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sv::block::Block;
use sv::transaction::Transaction as SvTransaction;
use sv::util::{deserialize as sv_deserialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use toml;
use tracing::{error, info, warn};

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTransactionConsensusRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTransactionConsensusResponse {
    success: bool,
    error: String,
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
struct GetPolicyRequest {}

#[derive(Serialize, Deserialize, Debug)]
struct GetPolicyResponse {
    max_block_size: u64,
    min_fee_rate: u64,
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
struct QueryUtxoRequest {
    txid: String,
    vout: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryUtxoResponse {
    exists: bool,
    script_pubkey: String,
    amount: u64,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageRequestType {
    QueryUtxo { request: QueryUtxoRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    QueryUtxo(QueryUtxoResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum ConsensusRequestType {
    ValidateTransaction { request: ValidateTransactionConsensusRequest, token: String },
    ValidateBlock { request: ValidateBlockConsensusRequest, token: String },
    GetPolicy { request: GetPolicyRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum ConsensusResponseType {
    ValidateTransaction(ValidateTransactionConsensusResponse),
    ValidateBlock(ValidateBlockConsensusResponse),
    GetPolicy(GetPolicyResponse),
    GetMetrics(GetMetricsResponse),
}

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

#[derive(Debug)]
struct ConsensusService {
    storage_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<gov::state::NotKeyed, gov::state::InMemoryState, gov::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl ConsensusService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("consensus_requests_total", "Total consensus requests").unwrap();
        let latency_ms = Gauge::new("consensus_latency_ms", "Average consensus request latency").unwrap();
        let errors_total = Counter::new("consensus_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        ConsensusService {
            storage_service_addr: env::var("STORAGE_ADDR").unwrap_or("127.0.0.1:50053".to_string()),
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            registry,
            requests_total,
            latency_ms,
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
            service: "consensus_service".to_string(),
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
            Ok(())
        } else {
            warn!("Alert sending failed: {}", response.error);
            Err(response.error)
        }
    }

    async fn query_utxo(&self, txid: &str, vout: u32, token: &str) -> Result<QueryUtxoResponse, String> {
        let mut stream = TcpStream::connect(&self.storage_service_addr).await
            .map_err(|e| format!("Failed to connect to storage_service: {}", e))?;
        let request = StorageRequestType::QueryUtxo { request: QueryUtxoRequest { txid: txid.to_string(), vout }, token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
        let response: StorageResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        match response {
            StorageResponseType::QueryUtxo(resp) => Ok(resp),
            _ => Err("Unexpected response type".to_string()),
        }
    }

    async fn handle_request(&self, request: ConsensusRequestType) -> Result<ConsensusResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            ConsensusRequestType::ValidateTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "ValidateTransaction").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let tx_bytes = hex::decode(&request.tx_hex).map_err(|e| {
                    warn!("Invalid transaction hex: {}", e);
                    format!("Invalid transaction hex: {}", e)
                })?;
                let tx: SvTransaction = sv_deserialize(&tx_bytes).map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    format!("Invalid transaction: {}", e)
                })?;

                // Real validation
                let mut input_sum = 0u64;
                let mut output_sum = 0u64;
                let mut seen_inputs = std::collections::HashSet::new();
                for input in &tx.inputs {
                    if !seen_inputs.insert((hex::encode(input.outpoint.tx_hash.0), input.outpoint.vout)) {
                        return Ok(ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
                            success: false,
                            error: "Double spend in tx".to_string(),
                        }));
                    }
                    let resp = self.query_utxo(&hex::encode(input.outpoint.tx_hash.0), input.outpoint.vout, &token).await?;
                    if !resp.exists {
                        return Ok(ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
                            success: false,
                            error: "Input UTXO not found".to_string(),
                        }));
                    }
                    input_sum += resp.amount;
                    // Script validation: Assume valid for now
                }
                for output in &tx.outputs {
                    output_sum += output.value;
                }
                if input_sum < output_sum {
                    return Ok(ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
                        success: false,
                        error: "Input sum less than output sum".to_string(),
                    }));
                }

                let success = true;
                let error = if success { "".to_string() } else { "Validation failed".to_string() };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
                    success,
                    error,
                }))
            }
            ConsensusRequestType::ValidateBlock { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "ValidateBlock").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let block_bytes = hex::decode(&request.block_hex).map_err(|e| {
                    warn!("Invalid block hex: {}", e);
                    format!("Invalid block hex: {}", e)
                })?;
                let block: Block = sv_deserialize(&block_bytes).map_err(|e| {
                    warn!("Invalid block: {}", e);
                    format!("Invalid block: {}", e)
                })?;

                // Real validation
                let success = block.header.validate_pow() && !block.txs.is_empty();
                let error = if success { "".to_string() } else { "Block validation failed".to_string() };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ConsensusResponseType::ValidateBlock(ValidateBlockConsensusResponse {
                    success,
                    error,
                }))
            }
            ConsensusRequestType::GetPolicy { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetPolicy").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ConsensusResponseType::GetPolicy(GetPolicyResponse {
                    max_block_size: 4_000_000_000,
                    min_fee_rate: 1,
                    error: "".to_string(),
                }))
            }
            ConsensusRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ConsensusResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "consensus_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: 0,
                    index_throughput: 0.0,
                }))
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Consensus service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: ConsensusRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
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

    let addr = env::var("CONSENSUS_ADDR").unwrap_or("127.0.0.1:50055".to_string());
    let consensus_service = ConsensusService::new().await;
    consensus_service.run(&addr).await?;
    Ok(())
}
