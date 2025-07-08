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
use sv::messages::Tx;
use sv::block::Block;
use sv::util::Serializable;
use sled::Db;
use std::io::Cursor;

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
struct QueryTransactionRequest {
    txid: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryTransactionResponse {
    success: bool,
    tx_hex: String,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct IndexTransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct IndexTransactionResponse {
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
enum IndexRequestType {
    QueryTransaction { request: QueryTransactionRequest, token: String },
    IndexTransaction { request: IndexTransactionRequest, token: String },
    QueryBlock { request: QueryBlockRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexResponseType {
    QueryTransaction(QueryTransactionResponse),
    IndexTransaction(IndexTransactionResponse),
    QueryBlock(QueryBlockResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct IndexService {
    auth_service_addr: String,
    alert_service_addr: String,
    db: Arc<Mutex<Db>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    index_throughput: Gauge,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl IndexService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let db = Arc::new(Mutex::new(
            sled::open("index_db").expect("Failed to open sled database"),
        ));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("index_requests_total", "Total index requests").unwrap();
        let latency_ms = Gauge::new("index_latency_ms", "Average index request latency").unwrap();
        let alert_count = Counter::new("index_alert_count", "Total alerts sent").unwrap();
        let index_throughput = Gauge::new("index_throughput", "Indexed transactions per second").unwrap();
        let errors_total = Counter::new("index_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(index_throughput.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        IndexService {
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
            db,
            registry,
            requests_total,
            latency_ms,
            alert_count,
            index_throughput,
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
            service: "index_service".to_string(),
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

    async fn handle_request(&self, request: IndexRequestType) -> Result<IndexResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            IndexRequestType::QueryTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "QueryTransaction").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let db = self.db.lock().await;
                let key = format!("tx:{}", request.txid);
                match db.get(key.as_bytes()) {
                    Ok(Some(value)) => {
                        let tx: Tx = Serializable::read(&mut Cursor::new(&value)).map_err(|e| {
                            warn!("Invalid transaction: {}", e);
                            let _ = self.send_alert("query_tx_invalid_deserialization", &format("Invalid transaction: {}", e), 2);
                            format("Invalid transaction: {}", e)
                        })?;
                        let mut tx_bytes = Vec::new();
                        tx.write(&mut tx_bytes).unwrap();
                        let tx_hex = hex::encode(&tx_bytes);
                        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        Ok(IndexResponseType::QueryTransaction(QueryTransactionResponse {
                            success: true,
                            tx_hex,
                            error: "".to_string(),
                        }))
                    }
                    Ok(None) => {
                        warn!("Transaction not found: {}", request.txid);
                        let _ = self.send_alert("query_tx_not_found", &format("Transaction not found: {}", request.txid), 2);
                        Ok(IndexResponseType::QueryTransaction(QueryTransactionResponse {
                            success: false,
                            tx_hex: "".to_string(),
                            error: "Transaction not found".to_string(),
                        }))
                    }
                    Err(e) => {
                        warn!("Failed to query transaction: {}", e);
                        let _ = self.send_alert("query_tx_failed", &format("Failed to query transaction: {}", e), 2);
                        Ok(IndexResponseType::QueryTransaction(QueryTransactionResponse {
                            success: false,
                            tx_hex: "".to_string(),
                            error: format("Failed to query transaction: {}", e),
                        }))
                    }
                }
            }
            IndexRequestType::IndexTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "IndexTransaction").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let tx_bytes = hex::decode(&request.tx_hex).map_err(|e| {
                    warn!("Invalid transaction hex: {}", e);
                    let _ = self.send_alert("index_tx_invalid_hex", &format("Invalid transaction hex: {}", e), 2);
                    format("Invalid transaction hex: {}", e)
                })?;
                let tx: Tx = Serializable::read(&mut Cursor::new(&tx_bytes)).map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    let _ = self.send_alert("index_tx_invalid_deserialization", &format("Invalid transaction: {}", e), 2);
                    format("Invalid transaction: {}", e)
                })?;

                let db = self.db.lock().await;
                let key = format!("tx:{}", hex::encode(tx.hash().0));
                let mut tx_bytes = Vec::new();
                tx.write(&mut tx_bytes).unwrap();
                db.insert(key.as_bytes(), &tx_bytes).map_err(|e| {
                    warn!("Failed to index transaction: {}", e);
                    let _ = self.send_alert("index_tx_failed", &format("Failed to index transaction: {}", e), 2);
                    format("Failed to index transaction: {}", e)
                })?;

                self.index_throughput.set(1.0); // Placeholder
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(IndexResponseType::IndexTransaction(IndexTransactionResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            IndexRequestType::QueryBlock { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "QueryBlock").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let db = self.db.lock().await;
                let key = format!("block:{}", request.block_hash);
                match db.get(key.as_bytes()) {
                    Ok(Some(value)) => {
                        let block: Block = Serializable::read(&mut Cursor::new(&value)).map_err(|e| {
                            warn!("Invalid block: {}", e);
                            let _ = self.send_alert("query_block_invalid_deserialization", &format("Invalid block: {}", e), 2);
                            format("Invalid block: {}", e)
                        })?;
                        let mut block_bytes = Vec::new();
                        block.write(&mut block_bytes).unwrap();
                        let block_hex = hex::encode(&block_bytes);
                        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        Ok(IndexResponseType::QueryBlock(QueryBlockResponse {
                            success: true,
                            block_hex,
                            error: "".to_string(),
                        }))
                    }
                    Ok(None) => {
                        warn!("Block not found: {}", request.block_hash);
                        let _ = self.send_alert("query_block_not_found", &format("Block not found: {}", request.block_hash), 2);
                        Ok(IndexResponseType::QueryBlock(QueryBlockResponse {
                            success: false,
                            block_hex: "".to_string(),
                            error: "Block not found".to_string(),
                        }))
                    }
                    Err(e) => {
                        warn!("Failed to query block: {}", e);
                        let _ = self.send_alert("query_block_failed", &format("Failed to query block: {}", e), 2);
                        Ok(IndexResponseType::QueryBlock(QueryBlockResponse {
                            success: false,
                            block_hex: "".to_string(),
                            error: format("Failed to query block: {}", e),
                        }))
                    }
                }
            }
            IndexRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(IndexResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "index_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: self.index_throughput.get(),
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
                                        let response = IndexResponseType::QueryTransaction(QueryTransactionResponse {
                                            success: false,
                                            tx_hex: "".to_string(),
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

    let addr = "127.0.0.1:50059";
    let index_service = IndexService::new().await;
    index_service.run(addr).await?;
    Ok(())
}
