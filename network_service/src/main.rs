use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use async_channel::{Sender, Receiver, unbounded};
use sv::messages::{Inv, InvVect, Hashable};
use sv::network::Network;
use hex::encode;
use governor::{Quota, RateLimiter};
use prometheus::{IntCounter, Gauge, Registry};
use jsonwebtoken::{decode, DecodingKey, Validation};
use std::sync::Arc;
use tokio::sync::Mutex;
use toml;
use shared::{ShardManager, Transaction};

#[derive(Serialize, Deserialize, Debug)]
enum NetworkRequest {
    Ping { message: String },
    DiscoverPeers,
    BroadcastTransaction { tx_hex: String },
    BroadcastBlock { block_hex: String },
    GetMetrics,
}

#[derive(Serialize, Deserialize, Debug)]
enum NetworkResponse {
    Ping { response: String, error: String },
    DiscoverPeers { peers: Vec<String>, error: String },
    BroadcastTransaction { success: bool, error: String },
    BroadcastBlock { success: bool, error: String },
    GetMetrics {
        service_name: String,
        requests_total: u64,
        avg_latency_ms: f64,
        errors_total: u64,
        cache_hits: u64,
        alert_count: u64,
        index_throughput: f64,
    },
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

struct NetworkService {
    transaction_service_addr: String,
    block_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    peers: Arc<Mutex<Vec<String>>>,
    registry: Arc<Registry>,
    requests_total: IntCounter,
    latency_ms: Gauge,
    alert_count: IntCounter,
    errors_total: IntCounter,
    rate_limiter: Arc<RateLimiter>,
    shard_manager: Arc<ShardManager>,
}

impl NetworkService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;
        let peers = Arc::new(Mutex::new(
            config["testnet"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        ));
        let registry = Arc::new(Registry::new());
        let requests_total = IntCounter::new("network_requests_total", "Total network requests").unwrap();
        let latency_ms = Gauge::new("network_latency_ms", "Average request latency (ms)").unwrap();
        let alert_count = IntCounter::new("network_alert_count", "Total alerts sent").unwrap();
        let errors_total = IntCounter::new("network_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            std::num::NonZeroU32::new(1000).unwrap(),
        )));

        NetworkService {
            transaction_service_addr: "127.0.0.1:50052".to_string(),
            block_service_addr: "127.0.0.1:50054".to_string(),
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
            peers,
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
            service: "network_service".to_string(),
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

    async fn handle_request(&self, request: NetworkRequest, token: Option<String>) -> Result<NetworkResponse, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        // Authenticate if token provided
        let user_id = if let Some(token) = token {
            self.authenticate(&token).await?
        } else {
            return Err("Missing token".to_string());
        };

        // Handle request
        match request {
            NetworkRequest::Ping { message } => {
                self.authorize(&user_id, "Ping").await?;
                self.rate_limiter.until_ready().await;
                let response = format!("Pong: {}", message);
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(NetworkResponse::Ping { response, error: "".to_string() })
            }
            NetworkRequest::DiscoverPeers => {
                self.authorize(&user_id, "DiscoverPeers").await?;
                self.rate_limiter.until_ready().await;
                let peers = self.peers.lock().await;
                let peer_list = peers.clone();
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(NetworkResponse::DiscoverPeers { peers: peer_list, error: "".to_string() })
            }
            NetworkRequest::BroadcastTransaction { tx_hex } => {
                self.authorize(&user_id, "BroadcastTransaction").await?;
                self.rate_limiter.until_ready().await;
                
                let tx_bytes = hex::decode(&tx_hex).map_err(|e| {
                    warn!("Invalid transaction hex: {}", e);
                    let _ = self.send_alert("broadcast_invalid_tx", &format!("Invalid transaction hex: {}", e), 2);
                    format!("Invalid transaction hex: {}", e)
                })?;
                let tx: Transaction = deserialize(&tx_bytes).map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    let _ = self.send_alert("broadcast_invalid_tx_deserialization", &format!("Invalid transaction: {}", e), 2);
                    format!("Invalid transaction: {}", e)
                })?;

                let mut stream = TcpStream::connect(&self.transaction_service_addr).await
                    .map_err(|e| format!("Failed to connect to transaction_service: {}", e))?;
                let tx_request = TransactionRequest { tx_hex: tx_hex.clone() };
                let encoded = serialize(&tx_request).map_err(|e| format!("Serialization error: {}", e))?;
                stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
                let tx_response: TransactionResponse = deserialize(&buffer[..n])
                    .map_err(|e| format!("Deserialization error: {}", e))?;

                if !tx_response.success {
                    warn!("Transaction processing failed: {}", tx_response.error);
                    let _ = self.send_alert("broadcast_tx_failed", &format!("Transaction processing failed: {}", tx_response.error), 2);
                    return Err(tx_response.error);
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(NetworkResponse::BroadcastTransaction { success: true, error: "".to_string() })
            }
            NetworkRequest::BroadcastBlock { block_hex } => {
                self.authorize(&user_id, "BroadcastBlock").await?;
                self.rate_limiter.until_ready().await;
                
                let block_bytes = hex::decode(&block_hex).map_err(|e| {
                    warn!("Invalid block hex: {}", e);
                    let _ = self.send_alert("broadcast_invalid_block", &format!("Invalid block hex: {}", e), 2);
                    format!("Invalid block hex: {}", e)
                })?;

                let mut stream = TcpStream::connect(&self.block_service_addr).await
                    .map_err(|e| format!("Failed to connect to block_service: {}", e))?;
                let block_request = BlockRequest { block_hex: block_hex.clone() };
                let encoded = serialize(&block_request).map_err(|e| format!("Serialization error: {}", e))?;
                stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
                let block_response: BlockResponse = deserialize(&buffer[..n])
                    .map_err(|e| format!("Deserialization error: {}", e))?;

                if !block_response.success {
                    warn!("Block validation failed: {}", block_response.error);
                    let _ = self.send_alert("broadcast_block_failed", &format!("Block validation failed: {}", block_response.error), 2);
                    return Err(block_response.error);
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(NetworkResponse::BroadcastBlock { success: true, error: "".to_string() })
            }
            NetworkRequest::GetMetrics => {
                self.authorize(&user_id, "GetMetrics").await?;
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(NetworkResponse::GetMetrics {
                    service_name: "network_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                })
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Network service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: NetworkRequest = match deserialize(&buffer[..n]) {
                                    Ok(req) => req,
                                    Err(e) => {
                                        error!("Deserialization error: {}", e);
                                        service.errors_total.inc();
                                        return;
                                    }
                                };
                                let token = None; // Token extraction needs client implementation
                                match service.handle_request(request, token).await {
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
                                        let response = NetworkResponse::Ping {
                                            response: "".to_string(),
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

    let addr = "127.0.0.1:50051";
    let network_service = NetworkService::new().await;
    network_service.run(addr).await?;
    Ok(())
}
