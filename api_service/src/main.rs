use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::Mutex;
use toml;
use governor::{Quota, RateLimiter};
use shared::ShardManager;
use sv::transaction::Transaction;
use sv::util::{deserialize as sv_deserialize, serialize as sv_serialize};

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
struct SubmitTransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct SubmitTransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryBlockRequest {
    block_hash: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryBlockResponse {
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
enum ApiRequestType {
    SubmitTransaction { request: SubmitTransactionRequest, token: String },
    QueryBlock { request: QueryBlockRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum ApiResponseType {
    SubmitTransaction(SubmitTransactionResponse),
    QueryBlock(QueryBlockResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum TransactionRequestType {
    ProcessTransaction { request: ProcessTransactionRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
struct ProcessTransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum TransactionResponseType {
    ProcessTransaction(ProcessTransactionResponse),
}

#[derive(Serialize, Deserialize, Debug)]
struct ProcessTransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexRequestType {
    QueryBlock { request: QueryBlockRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexResponseType {
    QueryBlock(QueryBlockResponse),
}

#[derive(Debug)]
struct ApiService {
    transaction_service_addr: String,
    index_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl ApiService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("api_requests_total", "Total API requests").unwrap();
        let latency_ms = Gauge::new("api_latency_ms", "Average API request latency").unwrap();
        let alert_count = Counter::new("api_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("api_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        ApiService {
            transaction_service_addr: "127.0.0.1:50052".to_string(),
            index_service_addr: "127.0.0.1:50059".to_string(),
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
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
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
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
            .map_err(|e| format("Failed to connect to auth_service: {}", e))?;
        let request = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "api_service".to_string(),
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
            .map_err(|e| format("Failed to connect to alert_service: {}", e))?;
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

    async fn handle_request(&self, request: ApiRequestType) -> Result<ApiResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            ApiRequestType::SubmitTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "SubmitTransaction").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let tx_bytes = hex::decode(&request.tx_hex).map_err(|e| {
                    warn!("Invalid transaction hex: {}", e);
                    let _ = self.send_alert("submit_tx_invalid_hex", &format("Invalid transaction hex: {}", e), 2);
                    format("Invalid transaction hex: {}", e)
                })?;
                let _tx: Transaction = sv_deserialize(&tx_bytes).map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    let _ = self.send_alert("submit_tx_invalid_deserialization", &format("Invalid transaction: {}", e), 2);
                    format("Invalid transaction: {}", e)
                })?;

                let mut stream = TcpStream::connect(&self.transaction_service_addr).await
                    .map_err(|e| format("Failed to connect to transaction_service: {}", e))?;
                let tx_request = TransactionRequestType::ProcessTransaction {
                    request: ProcessTransactionRequest { tx_hex: request.tx_hex.clone() },
                    token: token.clone(),
                };
                let encoded = serialize(&tx_request).map_err(|e| format("Serialization error: {}", e))?;
                stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
                let tx_response: TransactionResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format("Deserialization error: {}", e))?;

                let tx_response = match tx_response {
                    TransactionResponseType::ProcessTransaction(resp) => resp,
                    _ => {
                        warn!("Unexpected response from transaction_service");
                        let _ = self.send_alert("submit_tx_failed", "Unexpected response from transaction_service", 2);
                        return Err("Unexpected response from transaction_service".to_string());
                    }
                };

                if !tx_response.success {
                    warn!("Transaction processing failed: {}", tx_response.error);
                    let _ = self.send_alert("submit_tx_failed", &format("Transaction processing failed: {}", tx_response.error), 2);
                    return Ok(ApiResponseType::SubmitTransaction(SubmitTransactionResponse {
                        success: false,
                        error: tx_response.error,
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ApiResponseType::SubmitTransaction(SubmitTransactionResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            ApiRequestType::QueryBlock { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "QueryBlock").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut stream = TcpStream::connect(&self.index_service_addr).await
                    .map_err(|e| format("Failed to connect to index_service: {}", e))?;
                let index_request = IndexRequestType::QueryBlock {
                    request: QueryBlockRequest { block_hash: request.block_hash.clone() },
                    token: token.clone(),
                };
                let encoded = serialize(&index_request).map_err(|e| format("Serialization error: {}", e))?;
                stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
                let index_response: IndexResponseType = deserialize(&buffer[..n])
                    .map_err(|e| format("Deserialization error: {}", e))?;

                let index_response = match index_response {
                    IndexResponseType::QueryBlock(resp) => resp,
                    _ => {
                        warn!("Unexpected response from index_service");
                        let _ = self.send_alert("query_block_failed", "Unexpected response from index_service", 2);
                        return Err("Unexpected response from index_service".to_string());
                    }
                };

                if !index_response.success {
                    warn!("Block query failed: {}", index_response.error);
                    let _ = self.send_alert("query_block_failed", &format("Block query failed: {}", index_response.error), 2);
                    return Ok(ApiResponseType::QueryBlock(QueryBlockResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: index_response.error,
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ApiResponseType::QueryBlock(QueryBlockResponse {
                    success: true,
                    block_hex: index_response.block_hex,
                    error: "".to_string(),
                }))
            }
            ApiRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ApiResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "api_service".to_string(),
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

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("API service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: ApiRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = ApiResponseType::SubmitTransaction(SubmitTransactionResponse {
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

    let addr = "127.0.0.1:50050";
    let api_service = ApiService::new().await;
    api_service.run(addr).await?;
    Ok(())
                            }
