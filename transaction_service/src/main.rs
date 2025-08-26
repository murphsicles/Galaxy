use std::env;
use std::num::NonZeroU32;
use std::sync::Arc;

use async_channel::{Receiver, Sender, unbounded};
use bincode::{deserialize, serialize};
use dotenv::dotenv;
use governor::{Quota, RateLimiter};
use hex;
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sv::transaction::Transaction as SvTransaction;
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
struct ValidateTransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ProcessTransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ProcessTransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchValidateTransactionRequest {
    tx_hexes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchValidateTransactionResponse {
    results: Vec<ValidateTransactionResponse>,
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
struct ValidateTransactionConsensusRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTransactionConsensusResponse {
    success: bool,
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
}

#[derive(Serialize, Deserialize, Debug)]
enum ConsensusResponseType {
    ValidateTransaction(ValidateTransactionConsensusResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexRequestType {
    IndexTransaction(IndexTransactionRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexResponseType {
    IndexTransaction(IndexTransactionResponse),
}

#[derive(Debug)]
struct TransactionService {
    storage_service_addr: String,
    consensus_service_addr: String,
    index_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    tx_queue: Sender<String>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    index_throughput: Gauge,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<NotKeyed, governor::state::InMemoryState, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl TransactionService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let (tx_queue, _) = unbounded();
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("transaction_requests_total", "Total transaction requests").unwrap();
        let latency_ms = Gauge::new("transaction_latency_ms", "Average transaction processing latency").unwrap();
        let alert_count = Counter::new("transaction_alert_count", "Total alerts sent").unwrap();
        let index_throughput = Gauge::new("transaction_index_throughput", "Indexed transactions per second").unwrap();
        let errors_total = Counter::new("transaction_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(index_throughput.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));

        TransactionService {
            storage_service_addr: env::var("STORAGE_ADDR").unwrap_or("127.0.0.1:50053".to_string()),
            consensus_service_addr: env::var("CONSENSUS_ADDR").unwrap_or("127.0.0.1:50055".to_string()),
            index_service_addr: env::var("INDEX_ADDR").unwrap_or("127.0.0.1:50059".to_string()),
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            tx_queue,
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
            service: "transaction_service".to_string(),
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

    async fn validate_transaction(&self, tx_hex: &str, token: &str) -> Result<ValidateTransactionResponse, String> {
        let tx_bytes = hex::decode(tx_hex).map_err(|e| {
            warn!("Invalid transaction hex: {}", e);
            let _ = self.send_alert("validate_invalid_tx", &format!("Invalid transaction hex: {}", e), 2);
            format!("Invalid transaction hex: {}", e)
        })?;
        let tx: SvTransaction = sv_deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self.send_alert("validate_invalid_tx_deserialization", &format!("Invalid transaction: {}", e), 2);
            format!("Invalid transaction: {}", e)
        })?;

        let mut stream = TcpStream::connect(&self.consensus_service_addr).await
            .map_err(|e| format!("Failed to connect to consensus_service: {}", e))?;
        let consensus_request = ConsensusRequestType::ValidateTransaction { request: ValidateTransactionConsensusRequest { tx_hex: tx_hex.to_string() }, token: token.to_string() };
        let encoded = serialize(&consensus_request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let consensus_response: ConsensusResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;

        if let ConsensusResponseType::ValidateTransaction(resp) = consensus_response {
            Ok(ValidateTransactionResponse {
                success: resp.success,
                error: resp.error,
            })
        } else {
            Err("Unexpected response type".to_string())
        }
    }

    async fn process_transaction(&self, tx_hex: &str, token: &str) -> Result<ProcessTransactionResponse, String> {
        let tx_bytes = hex::decode(tx_hex).map_err(|e| {
            warn!("Invalid transaction hex: {}", e);
            let _ = self.send_alert("process_invalid_tx", &format!("Invalid transaction hex: {}", e), 2);
            format!("Invalid transaction hex: {}", e)
        })?;
        let tx: SvTransaction = sv_deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self.send_alert("process_invalid_tx_deserialization", &format!("Invalid transaction: {}", e), 2);
            format!("Invalid transaction: {}", e)
        })?;

        // Add to storage (e.g., UTXO updates)
        let mut stream = TcpStream::connect(&self.storage_service_addr).await
            .map_err(|e| format!("Failed to connect to storage_service: {}", e))?;
        let storage_request = StorageRequestType::BatchAddUtxo { request: BatchAddUtxoRequest { utxos: tx.outputs.iter().enumerate().map(|(vout, output)| AddUtxoRequest {
            txid: hex::encode(tx.txid().0),
            vout: vout as u32,
            script_pubkey: hex::encode(output.script_pubkey.clone()),
            amount: output.value,
        }).collect() }, token: token.to_string() };
        let encoded = serialize(&storage_request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let storage_response: StorageResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;

        if let StorageResponseType::BatchAddUtxo(resp) = storage_response {
            if resp.results.iter().any(|r| !r.success) {
                return Ok(ProcessTransactionResponse {
                    success: false,
                    error: "Failed to add UTXOs".to_string(),
                });
            }
        } else {
            return Err("Unexpected response type".to_string());
        }

        self.tx_queue.send(tx_hex.to_string()).await.map_err(|e| format!("Queue error: {}", e))?;

        Ok(ProcessTransactionResponse {
            success: true,
            error: "".to_string(),
        })
    }

    async fn index_transaction(&self, tx_hex: &str, token: &str) -> Result<IndexTransactionResponse, String> {
        let mut stream = TcpStream::connect(&self.index_service_addr).await
            .map_err(|e| format!("Failed to connect to index_service: {}", e))?;
        let index_request = IndexRequestType::IndexTransaction(IndexTransactionRequest { tx_hex: tx_hex.to_string() });
        let encoded = serialize(&index_request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let index_response: IndexResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;

        if let IndexResponseType::IndexTransaction(resp) = index_response {
            Ok(resp)
        } else {
            Err("Unexpected response type".to_string())
        }
    }

    async fn handle_request(&self, request: TransactionRequestType) -> Result<TransactionResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            TransactionRequestType::ValidateTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "ValidateTransaction").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let resp = self.validate_transaction(&request.tx_hex, &token).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TransactionResponseType::ValidateTransaction(resp))
            }
            TransactionRequestType::ProcessTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "ProcessTransaction").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let resp = self.process_transaction(&request.tx_hex, &token).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TransactionResponseType::ProcessTransaction(resp))
            }
            TransactionRequestType::BatchValidateTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "BatchValidateTransaction").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut results = vec![];
                for tx_hex in request.tx_hexes {
                    let resp = self.validate_transaction(&tx_hex, &token).await?;
                    results.push(resp);
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TransactionResponseType::BatchValidateTransaction(BatchValidateTransactionResponse { results }))
            }
            TransactionRequestType::IndexTransaction { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "IndexTransaction").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let resp = self.index_transaction(&request.tx_hex, &token).await?;

                self.index_throughput.set(1.0); // Update with actual throughput calculation if needed
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TransactionResponseType::IndexTransaction(resp))
            }
            TransactionRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(TransactionResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "transaction_service".to_string(),
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
        info!("Transaction service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: TransactionRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = TransactionResponseType::ValidateTransaction(ValidateTransactionResponse {
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

    let addr = env::var("TRANSACTION_ADDR").unwrap_or("127.0.0.1:50052".to_string());
    let transaction_service = TransactionService::new().await;
    transaction_service.run(&addr).await?;
    Ok(())
}
