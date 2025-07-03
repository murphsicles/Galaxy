use tonic::{transport::{Server, Channel}, Request, Response, Status, Streaming};
use network::network_server::{Network, NetworkServer};
use network::{
    PingRequest, PingResponse, DiscoverPeersRequest, DiscoverPeersResponse,
    BroadcastTransactionRequest, BroadcastTransactionResponse, BroadcastBlockRequest,
    BroadcastBlockResponse, GetMetricsRequest, GetMetricsResponse
};
use transaction::transaction_client::TransactionClient;
use block::block_client::BlockClient;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use toml;
use std::sync::Arc;
use tokio::sync::Mutex;

tonic::include_proto!("network");
tonic::include_proto!("transaction");
tonic::include_proto!("block");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct NetworkServiceImpl {
    transaction_client: TransactionClient<Channel>,
    block_client: BlockClient<Channel>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    peers: Arc<Mutex<Vec<String>>>,
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
        let shard_id = config["sharding"]["shard_id"]
            .as_integer()
            .unwrap_or(0) as u32;

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
        let peers = Arc::new(Mutex::new(
            config["testnet"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        ));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("network_requests_total", "Total network requests")
            .unwrap();
        let latency_ms = Gauge::new("network_latency_ms", "Average request latency (ms)")
            .unwrap();
        let alert_count = Counter::new("network_alert_count", "Total alerts sent")
            .unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(
            Quota::per_second(NonZeroU32::new(1000).unwrap()),
        ));
        let shard_manager = Arc::new(ShardManager::new());

        NetworkServiceImpl {
            transaction_client,
            block_client,
            auth_client,
            alert_client,
            peers,
            registry,
            requests_total,
            latency_ms,
            alert_count,
            rate_limiter,
            shard_manager,
        }
    }

    async fn authenticate(&self, token: &str) -> Result<String, Status> {
        let auth_request = AuthenticateRequest {
            token: token.to_string(),
        };
        let auth_response = self
            .auth_client
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
        let auth_response = self
            .auth_client
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
        let alert_response = self
            .alert_client
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
}

#[tonic::async_trait]
impl Network for NetworkServiceImpl {
    async fn ping(
        &self,
        request: Request<PingRequest>,
    ) -> Result<Response<PingResponse>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self
            .authenticate(
                token
                    .to_str()
                    .map_err(|e| Status::invalid_argument("Invalid token format"))?,
            )
            .await?;
        self.authorize(&user_id, "Ping").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let response = format!("Pong: {}", req.message);

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(PingResponse {
            response,
            error: "".to_string(),
        }))
    }

    async fn discover_peers(
        &self,
        request: Request<DiscoverPeersRequest>,
    ) -> Result<Response<DiscoverPeersResponse>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self
            .authenticate(
                token
                    .to_str()
                    .map_err(|e| Status::invalid_argument("Invalid token format"))?,
            )
            .await?;
        self.authorize(&user_id, "DiscoverPeers").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let peers = self.peers.lock().await;
        let peer_list = peers.clone();

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(DiscoverPeersResponse {
            peers: peer_list,
            error: "".to_string(),
        }))
    }

    async fn broadcast_transaction(
        &self,
        request: Request<BroadcastTransactionRequest>,
    ) -> Result<Response<BroadcastTransactionResponse>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self
            .authenticate(
                token
                    .to_str()
                    .map_err(|e| Status::invalid_argument("Invalid token format"))?,
            )
            .await?;
        self.authorize(&user_id, "BroadcastTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| {
                warn!("Invalid transaction hex: {}", e);
                let _ = self
                    .send_alert(
                        "broadcast_invalid_tx",
                        &format!("Invalid transaction hex: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid transaction hex: {}", e))
            })?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| {
                warn!("Invalid transaction: {}", e);
                let _ = self
                    .send_alert(
                        "broadcast_invalid_tx_deserialization",
                        &format!("Invalid transaction: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

        let tx_request = transaction::ProcessTransactionRequest {
            tx_hex: req.tx_hex.clone(),
        };
        let tx_response = self
            .transaction_client
            .process_transaction(tx_request)
            .await
            .map_err(|e| {
                warn!("Failed to process transaction: {}", e);
                let _ = self
                    .send_alert(
                        "broadcast_tx_process_failed",
                        &format!("Failed to process transaction: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to process transaction: {}", e))
            })?
            .into_inner();

        if !tx_response.success {
            warn!("Transaction processing failed: {}", tx_response.error);
            let _ = self
                .send_alert(
                    "broadcast_tx_failed",
                    &format!("Transaction processing failed: {}", tx_response.error),
                    2,
                )
                .await;
            return Err(Status::internal(tx_response.error));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BroadcastTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn broadcast_block(
        &self,
        request: Request<BroadcastBlockRequest>,
    ) -> Result<Response<BroadcastBlockResponse>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self
            .authenticate(
                token
                    .to_str()
                    .map_err(|e| Status::invalid_argument("Invalid token format"))?,
            )
            .await?;
        self.authorize(&user_id, "BroadcastBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| {
                warn!("Invalid block hex: {}", e);
                let _ = self
                    .send_alert(
                        "broadcast_invalid_block",
                        &format!("Invalid block hex: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid block hex: {}", e))
            })?;

        let block_request = block::ValidateBlockRequest {
            block_hex: req.block_hex.clone(),
        };
        let block_response = self
            .block_client
            .validate_block(block_request)
            .await
            .map_err(|e| {
                warn!("Failed to validate block: {}", e);
                let _ = self
                    .send_alert(
                        "broadcast_block_validation_failed",
                        &format!("Failed to validate block: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to validate block: {}", e))
            })?
            .into_inner();

        if !block_response.success {
            warn!("Block validation failed: {}", block_response.error);
            let _ = self
                .send_alert(
                    "broadcast_block_failed",
                    &format!("Block validation failed: {}", block_response.error),
                    2,
                )
                .await;
            return Err(Status::internal(block_response.error));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BroadcastBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_metrics(
        &self,
        request: Request<GetMetricsRequest>,
    ) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self
            .authenticate(
                token
                    .to_str()
                    .map_err(|e| Status::invalid_argument("Invalid token format"))?,
            )
            .await?;
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
