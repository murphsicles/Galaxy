use std::collections::HashSet;
use std::env;
use std::io::Cursor;
use std::num::NonZeroU32;
use std::sync::Arc;

use bincode::{deserialize, serialize};
use dotenv::dotenv;
use governor::{Quota, RateLimiter};
use hex;
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sv::messages::{Inv, InvVect, VerAck, Version};
use sv::util::{Hash256, Serializable};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
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
struct NetworkRequest {
    ping: Option<PingRequest>,
    discover_peers: Option<DiscoverPeersRequest>,
    broadcast_transaction: Option<BroadcastTransactionRequest>,
    broadcast_block: Option<BroadcastBlockRequest>,
    get_metrics: Option<GetMetricsRequest>,
}

#[derive(Serialize, Deserialize, Debug)]
struct NetworkResponse {
    ping: Option<PingResponse>,
    discover_peers: Option<DiscoverPeersResponse>,
    broadcast_transaction: Option<BroadcastTransactionResponse>,
    broadcast_block: Option<BroadcastBlockResponse>,
    get_metrics: Option<GetMetricsResponse>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PingRequest {
    message: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct PingResponse {
    response: String,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct DiscoverPeersRequest {
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct DiscoverPeersResponse {
    peers: Vec<String>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTransactionRequest {
    tx_hex: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastBlockRequest {
    block_hex: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastBlockResponse {
    success: bool,
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
struct TransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct TransactionResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BlockRequest {
    block_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BlockResponse {
    success: bool,
    error: String,
}

#[derive(Clone)]
struct NetworkService {
    transaction_service_addr: String,
    block_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    peers: Arc<Mutex<HashSet<String>>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<gov::state::NotKeyed, gov::state::InMemoryState, gov::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl NetworkService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("network_requests_total", "Total network requests").unwrap();
        let latency_ms = Gauge::new("network_latency_ms", "Average request latency (ms)").unwrap();
        let alert_count = Counter::new("network_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("network_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        let service = NetworkService {
            transaction_service_addr: env::var("TRANSACTION_ADDR").unwrap_or("127.0.0.1:50052".to_string()),
            block_service_addr: env::var("BLOCK_ADDR").unwrap_or("127.0.0.1:50054".to_string()),
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            peers: Arc::new(Mutex::new(HashSet::new())),
            registry,
            requests_total,
            latency_ms,
            alert_count,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
        };

        let service_clone = service.clone();
        tokio::spawn(async move {
            service_clone.discover_peers().await;
        });

        service
    }

    async fn discover_peers(&self) {
        let testnet_ips = vec![
            "52.16.212.66:18333".to_string(),
            "23.22.19.204:18333".to_string(),
            "3.123.101.88:18333".to_string(),
            "54.152.215.212:18333".to_string(),
        ];

        let mut peers = self.peers.lock().await;
        for ip in testnet_ips {
            peers.insert(ip);
        }
    }

    async fn connect_to_peer(&self, addr: &str) -> Result<TcpStream, String> {
        let mut stream = TcpStream::connect(addr).await.map_err(|e| format!("Connect error: {}", e))?;
        // Send Version message
        let version = Version {
            version: 70015,
            services: 1,
            timestamp: Utc::now().timestamp() as u64,
            receiver_services: 1,
            receiver_address: "127.0.0.1".to_string(),
            receiver_port: 18333,
            sender_services: 1,
            sender_address: "127.0.0.1".to_string(),
            sender_port: 18333,
            nonce: 0,
            user_agent: "/Galaxy:0.2.3/".to_string(),
            start_height: 0,
            relay: true,
        };
        let encoded = version.serialize();
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        // Receive Version
        let mut buffer = vec![0u8; 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let _received_version: Version = Serializable::read(&mut Cursor::new(&buffer[..n])).map_err(|e| format!("Parse error: {}", e))?;

        // Send VerAck
        let verack = VerAck {};
        let encoded = verack.serialize();
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        // Receive VerAck
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let _received_verack: VerAck = Serializable::read(&mut Cursor::new(&buffer[..n])).map_err(|e| format!("Parse error: {}", e))?;

        Ok(stream)
    }

    async fn broadcast_to_peers(&self, msg: &[u8]) -> Result<(), String> {
        let peers = self.peers.lock().await.clone();
        for peer in peers {
            if let Ok(mut stream) = self.connect_to_peer(&peer).await {
                stream.write_all(msg).await.map_err(|e| format!("Write error: {}", e))?;
                stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;
            }
        }
        Ok(())
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

    async fn handle_request(&self, request: NetworkRequest) -> Result<NetworkResponse, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        if let Some(ping_request) = request.ping {
            let user_id = self.authenticate(&ping_request.token).await?;
            self.authorize(&user_id, "Ping").await?;
            self.rate_limiter.until_ready().await;
            let response = format!("Pong: {}", ping_request.message);
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            Ok(NetworkResponse {
                ping: Some(PingResponse { response, error: "".to_string() }),
                discover_peers: None,
                broadcast_transaction: None,
                broadcast_block: None,
                get_metrics: None,
            })
        } else if let Some(discover_peers) = request.discover_peers {
            let user_id = self.authenticate(&discover_peers.token).await?;
            self.authorize(&user_id, "DiscoverPeers").await?;
            self.rate_limiter.until_ready().await;
            let peers = self.peers.lock().await;
            let peer_list = peers.iter().cloned().collect();
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            Ok(NetworkResponse {
                ping: None,
                discover_peers: Some(DiscoverPeersResponse { peers: peer_list, error: "".to_string() }),
                broadcast_transaction: None,
                broadcast_block: None,
                get_metrics: None,
            })
        } else if let Some(broadcast_transaction) = request.broadcast_transaction {
            let user_id = self.authenticate(&broadcast_transaction.token).await?;
            self.authorize(&user_id, "BroadcastTransaction").await?;
            self.rate_limiter.until_ready().await;
            
            let tx_bytes = hex::decode(&broadcast_transaction.tx_hex).map_err(|e| {
                warn!("Invalid transaction hex: {}", e);
                let _ = self.send_alert("broadcast_invalid_tx", &format!("Invalid transaction hex: {}", e), 2);
                format!("Invalid transaction hex: {}", e)
            })?;
            let tx: SvTransaction = deserialize(&tx_bytes).map_err(|e| {
                warn!("Invalid transaction: {}", e);
                let _ = self.send_alert("broadcast_invalid_tx_deserialization", &format!("Invalid transaction: {}", e), 2);
                format!("Invalid transaction: {}", e)
            })?;

            let mut stream = TcpStream::connect(&self.transaction_service_addr).await
                .map_err(|e| format!("Failed to connect to transaction_service: {}", e))?;
            let tx_request = TransactionRequest { tx_hex: broadcast_transaction.tx_hex.clone() };
            let encoded = serialize(&tx_request).map_err(|e| format!("Serialization error: {}", e))?;
            stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
            stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
            let tx_response: TransactionResponse = deserialize(&buffer[..n])
                .map_err(|e| format!("Deserialization error: {}", e))?;

            if !tx_response.success {
                warn!("Transaction processing failed: {}", tx_response.error);
                let _ = self.send_alert("broadcast_tx_failed", &format!("Transaction processing failed: {}", tx_response.error), 2);
                return Err(tx_response.error);
            }

            // Broadcast to peers
            let inv = Inv {
                inventory: vec![InvVect {
                    hash: Hash256::from(tx.txid().0),
                    inv_type: 1, // TX
                }],
            };
            let encoded_inv = inv.serialize();
            let _ = self.broadcast_to_peers(&encoded_inv).await;

            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            Ok(NetworkResponse {
                ping: None,
                discover_peers: None,
                broadcast_transaction: Some(BroadcastTransactionResponse { success: true, error: "".to_string() }),
                broadcast_block: None,
                get_metrics: None,
            })
        } else if let Some(broadcast_block) = request.broadcast_block {
            let user_id = self.authenticate(&broadcast_block.token).await?;
            self.authorize(&user_id, "BroadcastBlock").await?;
            self.rate_limiter.until_ready().await;
            
            let block_bytes = hex::decode(&broadcast_block.block_hex).map_err(|e| {
                warn!("Invalid block hex: {}", e);
                let _ = self.send_alert("broadcast_invalid_block", &format!("Invalid block hex: {}", e), 2);
                format!("Invalid block hex: {}", e)
            })?;

            let mut stream = TcpStream::connect(&self.block_service_addr).await
                .map_err(|e| format!("Failed to connect to block_service: {}", e))?;
            let block_request = BlockRequest { block_hex: broadcast_block.block_hex.clone() };
            let encoded = serialize(&block_request).map_err(|e| format!("Serialization error: {}", e))?;
            stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
            stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
            let block_response: BlockResponse = deserialize(&buffer[..n])
                .map_err(|e| format!("Deserialization error: {}", e))?;

            if !block_response.success {
                warn!("Block validation failed: {}", block_response.error);
                let _ = self.send_alert("broadcast_block_failed", &format!("Block validation failed: {}", block_response.error), 2);
                return Err(block_response.error);
            }

            // Broadcast to peers
            let block: Block = sv::util::deserialize(&block_bytes).map_err(|e| format!("Deserialization error: {}", e))?;
            let inv = Inv {
                inventory: vec![InvVect {
                    hash: block.header.hash(),
                    inv_type: 2, // BLOCK
                }],
            };
            let encoded_inv = inv.serialize();
            let _ = self.broadcast_to_peers(&encoded_inv).await;

            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            Ok(NetworkResponse {
                ping: None,
                discover_peers: None,
                broadcast_transaction: None,
                broadcast_block: Some(BroadcastBlockResponse { success: true, error: "".to_string() }),
                get_metrics: None,
            })
        } else if let Some(get_metrics) = request.get_metrics {
            let user_id = self.authenticate(&get_metrics.token).await?;
            self.authorize(&user_id, "GetMetrics").await?;
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            Ok(NetworkResponse {
                ping: None,
                discover_peers: None,
                broadcast_transaction: None,
                broadcast_block: None,
                get_metrics: Some(GetMetricsResponse {
                    service_name: "network_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                }),
            })
        } else {
            Err("Invalid request".to_string())
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Network service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self.clone();
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
                                        let response = NetworkResponse {
                                            ping: Some(PingResponse { response: "".to_string(), error: e }),
                                            discover_peers: None,
                                            broadcast_transaction: None,
                                            broadcast_block: None,
                                            get_metrics: None,
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

    let addr = env::var("NETWORK_ADDR").unwrap_or("127.0.0.1:50051".to_string());
    let network_service = NetworkService::new().await;
    network_service.run(&addr).await?;
    Ok(())
}
