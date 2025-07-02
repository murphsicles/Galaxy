use tonic::{transport::{Server, Channel}, Request, Response, Status};
use network::network_server::{Network, NetworkServer};
use network::{
    PingRequest, PingResponse, DiscoverPeersRequest, DiscoverPeersResponse,
    BroadcastTxRequest, BroadcastTxResponse, BroadcastBlockRequest, BroadcastBlockResponse
};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use block::block_client::BlockClient;
use block::ValidateBlockRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::messages::{Message, NetworkMessage};
use sv::network::{Network as BSVNetwork, Version};
use sv::transaction::Transaction;
use sv::block::Block;
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::mpsc;
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter, Jitter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use hex;
use toml;

tonic::include_proto!("network");
tonic::include_proto!("transaction");
tonic::include_proto!("block");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct NetworkServiceImpl {
    peers: Arc<Mutex<HashMap<String, mpsc::Sender<Message>>>>,
    transaction_client: TransactionClient<Channel>,
    block_client: BlockClient<Channel>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl NetworkServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let initial_peers = config["testnet"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect::<Vec<String>>();
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let peers = Arc::new(Mutex::new(HashMap::new()));
        let transaction_client = TransactionClient::connect("http://[::1]:50052")
            .await
            .expect("Failed to connect to transaction_service");
        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("network_requests_total", "Total network requests").unwrap();
        let latency_ms = Gauge::new("network_latency_ms", "Average request latency").unwrap();
        let alert_count = Counter::new("network_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));
        let shard_manager = Arc::new(ShardManager::new());

        let peers_clone = Arc::clone(&peers);
        tokio::spawn(async move {
            for peer in initial_peers {
                if let Ok(stream) = TcpStream::connect(&peer).await {
                    let (tx, mut rx) = mpsc::channel(100);
                    {
                        let mut peers = peers_clone.lock().await;
                        peers.insert(peer.clone(), tx);
                    }
                    tokio::spawn(Self::handle_peer(stream, peer, peers_clone.clone()));
                }
            }
        });

        NetworkServiceImpl {
            peers,
            transaction_client,
            block_client,
            auth_client,
            alert_client,
            registry,
            requests_total,
            latency_ms,
            alert_count,
            rate_limiter,
            shard_manager,
        }
    }

    async fn authenticate(&self, token: &str) -> Result<String, Status> {
        let auth_request = AuthenticateRequest { token: token.to_string() };
        let auth_response = self.auth_client
            .authenticate(auth_request)
            .await
            .map_err(|e| Status::unauthenticated(format!("Authentication failed: {}", e)))?
            .into_inner();
        if !auth_response.success {
            return Err(Status::unauthenticated(auth_response.error));
        }
        Ok(auth_response.user_id)
    }

    async fn authorize(&self, user_id: &str, method: &str) -> Result<(), Status> {
        let auth_request = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "network_service".to_string(),
            method: method.to_string(),
        };
        let auth_response = self.auth_client
            .authorize(auth_request)
            .await
            .map_err(|e| Status::permission_denied(format!("Authorization failed: {}", e)))?
            .into_inner();
        if !auth_response.allowed {
            return Err(Status::permission_denied(auth_response.error));
        }
        Ok(())
    }

    async fn send_alert(&self, event_type: &str, message: &str, severity: u32) -> Result<(), Status> {
        let alert_request = SendAlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let alert_response = self.alert_client
            .send_alert(alert_request)
            .await
            .map_err(|e| {
                warn!("Failed to send alert: {}", e);
                Status::internal(format!("Failed to send alert: {}", e))
            })?
            .into_inner();
        if !alert_response.success {
            warn!("Alert sending failed: {}", alert_response.error);
            return Err(Status::internal(alert_response.error));
        }
        self.alert_count.inc();
        Ok(())
    }

    async fn send_version_message(stream: &mut TcpStream, addr: &str) -> Result<(), String> {
        info!("Sending version message to peer: {}", addr);
        let version = Version {
            version: 70015,
            services: 0,
            timestamp: 0,
            receiver_services: 0,
            receiver_ip: [0; 16],
            receiver_port: 0,
            sender_services: 0,
            sender_ip: [0; 16],
            sender_port: 0,
            nonce: 0,
            user_agent: "/Galaxy:0.1.0/".to_string(),
            start_height: 0,
            relay: false,
        };
        let message = Message {
            command: "version".to_string(),
            payload: version.serialize(),
        };
        let serialized = message.serialize(BSVNetwork::Testnet);
        stream.write_all(&serialized).await.map_err(|e| {
            warn!("Failed to send version message to {}: {}", addr, e);
            e.to_string()
        })?;

        let mut buffer = vec![0u8; 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| {
            warn!("Failed to read from {}: {}", addr, e);
            e.to_string()
        })?;
        let response = Message::deserialize(&buffer[..n], BSVNetwork::Testnet)
            .map_err(|e| {
                warn!("Failed to deserialize response from {}: {}", addr, e);
                e.to_string()
            })?;
        if response.command == "verack" {
            info!("Received verack from {}", addr);
            Ok(())
        } else {
            warn!("Expected verack from {}, got {}", addr, response.command);
            Err("Expected Verack".to_string())
        }
    }

    async fn handle_peer(stream: TcpStream, addr: String, peers: Arc<Mutex<HashMap<String, mpsc::Sender<Message>>>>) {
        let mut stream = stream;
        if Self::send_version_message(&mut stream, &addr).await.is_err() {
            let mut peers = peers.lock().await;
            peers.remove(&addr);
            warn!("Removed peer {} due to failed version handshake", addr);
            return;
        }

        let mut buffer = vec![0u8; 1024];
        loop {
            match stream.read(&mut buffer).await {
                Ok(n) if n > 0 => {
                    if let Ok(message) = Message::deserialize(&buffer[..n], BSVNetwork::Testnet) {
                        match message.command.as_str() {
                            "addr" => info!("Received addr message from {}", addr),
                            "inv" => info!("Received inv message from {}", addr),
                            "getdata" => info!("Received getdata message from {}", addr),
                            _ => {}
                        }
                    }
                }
                _ => {
                    let mut peers = peers.lock().await;
                    peers.remove(&addr);
                    warn!("Removed peer {} due to connection error", addr);
                    break;
                }
            }
        }
    }

    async fn broadcast_message(&self, message: Message) -> Result<(), String> {
        let peers = self.peers.lock().await;
        for (addr, tx) in peers.iter() {
            if let Err(e) = tx.send(message.clone()).await {
                warn!("Failed to send message to {}: {}", addr, e);
            }
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl Network for NetworkServiceImpl {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "Ping").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Processing ping request: {}", request.get_ref().message);
        let req = request.into_inner();
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(PingResponse {
            reply: format!("Pong: {}", req.message),
        }))
    }

    async fn discover_peers(&self, request: Request<DiscoverPeersRequest>) -> Result<Response<DiscoverPeersResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "DiscoverPeers").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Discovering peers");
        let peers = self.peers.lock().await;
        let peer_addresses: Vec<String> = peers.keys().cloned().collect();
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        if peer_addresses.is_empty() {
            let _ = self.send_alert("peer_discovery_failed", "No active peers found", 2).await;
            warn!("No active peers found");
        }
        info!("Discovered {} peers", peer_addresses.len());
        Ok(Response::new(DiscoverPeersResponse {
            peer_addresses,
            error: if peer_addresses.is_empty() { "No active peers".to_string() } else { "".to_string() },
        }))
    }

    async fn broadcast_transaction(&self, request: Request<BroadcastTxRequest>) -> Result<Response<BroadcastTxResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BroadcastTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Broadcasting transaction: {}", request.get_ref().tx_hex);
        let req = request.into_inner();
        let mut client = self.transaction_client.clone();
        let validate_request = ValidateTxRequest {
            tx_hex: req.tx_hex.clone(),
        };
        let validate_response = client
            .validate_transaction(validate_request)
            .await
            .map_err(|e| {
                warn!("Transaction validation failed: {}", e);
                Status::internal(format!("Validation failed: {}", e))
            })?
            .into_inner();

        if !validate_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Invalid transaction: {}", validate_response.error);
            let _ = self.send_alert("tx_broadcast_validation_failed", &format!("Transaction validation failed: {}", validate_response.error), 2).await;
            return Ok(Response::new(BroadcastTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| {
                warn!("Invalid tx_hex: {}", e);
                Status::invalid_argument(format!("Invalid tx_hex: {}", e))
            })?;
        let tx: Transaction = sv::util::deserialize(&tx_bytes)
            .map_err(|e| {
                warn!("Invalid transaction: {}", e);
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

        let message = Message {
            command: "tx".to_string(),
            payload: tx.serialize(),
        };
        self.broadcast_message(message).await
            .map_err(|e| {
                warn!("Broadcast failed: {}", e);
                Status::internal(format!("Broadcast failed: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully broadcast transaction: {}", req.tx_hex);
        Ok(Response::new(BroadcastTxResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn broadcast_block(&self, request: Request<BroadcastBlockRequest>) -> Result<Response<BroadcastBlockResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BroadcastBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Broadcasting block: {}", request.get_ref().block_hex);
        let req = request.into_inner();
        let mut client = self.block_client.clone();
        let validate_request = ValidateBlockRequest {
            block_hex: req.block_hex.clone(),
        };
        let validate_response = client
            .validate_block(validate_request)
            .await
            .map_err(|e| {
                warn!("Block validation failed: {}", e);
                Status::internal(format!("Block validation failed: {}", e))
            })?
            .into_inner();

        if !validate_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Invalid block: {}", validate_response.error);
            let _ = self.send_alert("block_broadcast_validation_failed", &format!("Block validation failed: {}", validate_response.error), 2).await;
            return Ok(Response::new(BroadcastBlockResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| {
                warn!("Invalid block_hex: {}", e);
                Status::invalid_argument(format!("Invalid block_hex: {}", e))
            })?;
        let block: Block = sv::util::deserialize(&block_bytes)
            .map_err(|e| {
                warn!("Invalid block: {}", e);
                Status::invalid_argument(format!("Invalid block: {}", e))
            })?;

        let message = Message {
            command: "block".to_string(),
            payload: block.serialize(),
        };
        self.broadcast_message(message).await
            .map_err(|e| {
                warn!("Broadcast failed: {}", e);
                Status::internal(format!("Broadcast failed: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully broadcast block: {}", req.block_hex);
        Ok(Response::new(BroadcastBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_metrics(&self, request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetMetrics").await?;

        Ok(Response::new(GetMetricsResponse {
            service_name: "network_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: 0, // Not applicable
            alert_count: self.alert_count.get() as u64,
            index_throughput: 0.0, // Not applicable
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "[::1]:50051".parse().unwrap();
    let network_service = NetworkServiceImpl::new().await;

    println!("Network service listening on {}", addr);

    Server::builder()
        .add_service(NetworkServer::new(network_service))
        .serve(addr)
        .await?;

    Ok(())
}
