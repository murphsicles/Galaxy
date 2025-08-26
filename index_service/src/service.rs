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
use sled::Db;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use toml;
use tracing::{error, info, warn};

use super::TxStatus;

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
struct IndexTransactionRequest {
    txid: String,
    status: TxStatus,
    block_hash: Option<String>,
    block_height: Option<u64>,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct IndexTransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetTxStatusRequest {
    txid: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetTxStatusResponse {
    status: TxStatus,
    block_hash: Option<String>,
    block_height: Option<u64>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsRequest {
    token: String,
}

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
enum IndexRequestType {
    IndexTransaction(IndexTransactionRequest),
    GetTxStatus(GetTxStatusRequest),
    GetMetrics(GetMetricsRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexResponseType {
    IndexTransaction(IndexTransactionResponse),
    GetTxStatus(GetTxStatusResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct IndexService {
    index_db: Arc<Db>,
    auth_service_addr: String,
    alert_service_addr: String,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<NotKeyed, governor::state::InMemoryState, DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl IndexService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");

        let index_db = sled::open("tx_index.db").expect("Failed to open sled DB");

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("index_requests_total", "Total index requests").unwrap();
        let latency_ms = Gauge::new("index_latency_ms", "Average index request latency").unwrap();
        let alert_count = Counter::new("index_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("index_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        IndexService {
            index_db: Arc::new(index_db),
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
            service: "index_service".to_string(),
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

    async fn handle_request(&self, request: IndexRequestType) -> Result<IndexResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            IndexRequestType::IndexTransaction(req) => {
                let user_id = self.authenticate(&req.token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "IndexTransaction").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let data = serialize(&(req.status, req.block_hash, req.block_height)).map_err(|e| format!("Serialization error: {}", e))?;
                self.index_db.insert(req.txid.as_bytes(), &data).map_err(|e| format!("Sled error: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(IndexResponseType::IndexTransaction(IndexTransactionResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            IndexRequestType::GetTxStatus(req) => {
                let user_id = self.authenticate(&req.token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetTxStatus").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                if let Some(data) = self.index_db.get(req.txid.as_bytes()).map_err(|e| format!("Sled error: {}", e))? {
                    let (status, block_hash, block_height) = deserialize(&data).map_err(|e| format!("Deserialization error: {}", e))?;
                    self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                    Ok(IndexResponseType::GetTxStatus(GetTxStatusResponse {
                        status,
                        block_hash,
                        block_height,
                        error: "".to_string(),
                    }))
                } else {
                    self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                    Ok(IndexResponseType::GetTxStatus(GetTxStatusResponse {
                        status: TxStatus::Unknown,
                        block_hash: None,
                        block_height: None,
                        error: "Tx not found".to_string(),
                    }))
                }
            }
            IndexRequestType::GetMetrics(req) => {
                let user_id = self.authenticate(&req.token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(IndexResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "index_service".to_string(),
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
        info!("Index service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: IndexRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = IndexResponseType::IndexTransaction(IndexTransactionResponse {
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

    let addr = env::var("INDEX_ADDR").unwrap_or("127.0.0.1:50059".to_string());
    let index_service = IndexService::new().await;
    index_service.run(&addr).await?;
    Ok(())
}
