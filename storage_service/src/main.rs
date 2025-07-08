use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::Mutex;
use toml;
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter};
use shared::ShardManager;
use tigerbeetle_unofficial as tigerbeetle;

// Temporary: Keep alert_client until alert_service is updated
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use tonic::transport::Channel;

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
struct AddUtxoRequest {
    txid: String,
    vout: u32,
    script_pubkey: String,
    amount: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct AddUtxoResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct RemoveUtxoRequest {
    txid: String,
    vout: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct RemoveUtxoResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchAddUtxoRequest {
    utxos: Vec<AddUtxoRequest>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchAddUtxoResponse {
    results: Vec<AddUtxoResponse>,
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
enum StorageRequestType {
    QueryUtxo { request: QueryUtxoRequest, token: String },
    AddUtxo { request: AddUtxoRequest, token: String },
    RemoveUtxo { request: RemoveUtxoRequest, token: String },
    BatchAddUtxo { request: BatchAddUtxoRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    QueryUtxo(QueryUtxoResponse),
    AddUtxo(AddUtxoResponse),
    RemoveUtxo(RemoveUtxoResponse),
    BatchAddUtxo(BatchAddUtxoResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct StorageService {
    utxos: Arc<Mutex<HashMap<String, (String, u64)>>>,
    auth_service_addr: String,
    alert_client: AlertClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl StorageService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let utxos = Arc::new(Mutex::new(HashMap::new()));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("storage_requests_total", "Total storage requests").unwrap();
        let latency_ms = Gauge::new("storage_latency_ms", "Average storage request latency").unwrap();
        let alert_count = Counter::new("storage_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("storage_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        StorageService {
            utxos,
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_client,
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
            .map_err(|e| format("Failed to connect to auth_service: {}", e))?;
        let request = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "storage_service".to_string(),
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
        let alert_request = SendAlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let alert_response = self
            .alert_client
            .send_alert(alert_request)
            .await
            .map_err(|e| {
                warn!("Failed to send alert: {}", e);
                format("Failed to send alert: {}", e)
            })?;
        let alert_response = alert_response.into_inner();
        if !alert_response.success {
            warn!("Alert sending failed: {}", alert_response.error);
            return Err(alert_response.error);
        }
        self.alert_count.inc();
        Ok(())
    }

    async fn handle_request(&self, request: StorageRequestType) -> Result<StorageResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            StorageRequestType::QueryUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "QueryUtxo").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let key = format!("{}:{}", request.txid, request.vout);
                let utxos = self.utxos.lock().await;
                let exists = utxos.contains_key(&key);

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::QueryUtxo(QueryUtxoResponse {
                    exists,
                    script_pubkey: utxos.get(&key).map(|(script, _)| script.clone()).unwrap_or_default(),
                    amount: utxos.get(&key).map(|(_, amount)| *amount).unwrap_or(0),
                    error: if exists { "".to_string() } else { "UTXO not found".to_string() },
                }))
            }
            StorageRequestType::AddUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "AddUtxo").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let key = format!("{}:{}", request.txid, request.vout);
                let mut utxos = self.utxos.lock().await;
                utxos.insert(key, (request.script_pubkey.clone(), request.amount));

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::AddUtxo(AddUtxoResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            StorageRequestType::RemoveUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "RemoveUtxo").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let key = format!("{}:{}", request.txid, request.vout);
                let mut utxos = self.utxos.lock().await;
                let removed = utxos.remove(&key).is_some();

                if !removed {
                    warn!("UTXO not found for txid: {}", request.txid);
                    let _ = self.send_alert("remove_utxo_not_found", &format("UTXO not found for txid: {}", request.txid), 2);
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::RemoveUtxo(RemoveUtxoResponse {
                    success: removed,
                    error: if removed { "".to_string() } else { "UTXO not found".to_string() },
                }))
            }
            StorageRequestType::BatchAddUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "BatchAddUtxo").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut utxos = self.utxos.lock().await;
                let mut results = vec![];

                for utxo in request.utxos {
                    let key = format!("{}:{}", utxo.txid, utxo.vout);
                    utxos.insert(key, (utxo.script_pubkey.clone(), utxo.amount));
                    results.push(AddUtxoResponse {
                        success: true,
                        error: "".to_string(),
                    });
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::BatchAddUtxo(BatchAddUtxoResponse { results }))
            }
            StorageRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "storage_service".to_string(),
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
        info!("Storage service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: StorageRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = StorageResponseType::QueryUtxo(QueryUtxoResponse {
                                            exists: false,
                                            script_pubkey: "".to_string(),
                                            amount: 0,
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

    let addr = "127.0.0.1:50053";
    let storage_service = StorageService::new().await;
    storage_service.run(addr).await?;
    Ok(())
}
