use tonic::{transport::{Server, Channel}, Request, Response, Status};
use block::block_server::{Block, BlockServer};
use block::{
    ValidateBlockRequest, ValidateBlockResponse, AssembleBlockRequest, AssembleBlockResponse,
    IndexBlockRequest, IndexBlockResponse, GetBlockHeadersRequest, GetBlockHeadersResponse,
    GetMetricsRequest, GetMetricsResponse
};
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateBlockConsensusRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::block::Block;
use sv::util::{deserialize, serialize};
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use toml;
use std::sync::Arc;
use tokio::sync::Mutex;

tonic::include_proto!("block");
tonic::include_proto!("consensus");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct BlockServiceImpl {
    consensus_client: ConsensusClient<Channel>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    blocks: Arc<Mutex<Vec<String>>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl BlockServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"]
            .as_integer()
            .unwrap_or(0) as u32;

        let consensus_client = ConsensusClient::connect("http://[::1]:50055")
            .await
            .expect("Failed to connect to consensus_service");
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let blocks = Arc::new(Mutex::new(Vec::new()));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("block_requests_total", "Total block requests")
            .unwrap();
        let latency_ms = Gauge::new("block_latency_ms", "Average block request latency")
            .unwrap();
        let alert_count = Counter::new("block_alert_count", "Total alerts sent")
            .unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(
            Quota::per_second(NonZeroU32::new(1000).unwrap()),
        ));
        let shard_manager = Arc::new(ShardManager::new());

        BlockServiceImpl {
            consensus_client,
            auth_client,
            alert_client,
            blocks,
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
            service: "block_service".to_string(),
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
impl Block for BlockServiceImpl {
    async fn validate_block(
        &self,
        request: Request<ValidateBlockRequest>,
    ) -> Result<Response<ValidateBlockResponse>, Status> {
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
        self.authorize(&user_id, "ValidateBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| {
                warn!("Invalid block hex: {}", e);
                let _ = self
                    .send_alert(
                        "validate_invalid_block",
                        &format!("Invalid block hex: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid block hex: {}", e))
            })?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| {
                warn!("Invalid block: {}", e);
                let _ = self
                    .send_alert(
                        "validate_invalid_block_deserialization",
                        &format!("Invalid block: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid block: {}", e))
            })?;

        if block.serialized_size() > 34_359_738_368 {
            warn!("Block size exceeds 32GB: {}", block.serialized_size());
            let _ = self
                .send_alert(
                    "validate_block_size_exceeded",
                    &format!("Block size exceeds 32GB: {}", block.serialized_size()),
                    3,
                )
                .await;
            return Ok(Response::new(ValidateBlockResponse {
                success: false,
                error: "Block size exceeds 32GB".to_string(),
            }));
        }

        let consensus_request = ValidateBlockConsensusRequest {
            block_hex: req.block_hex.clone(),
        };
        let consensus_response = self
            .consensus_client
            .validate_block_consensus(consensus_request)
            .await
            .map_err(|e| {
                warn!("Consensus validation failed: {}", e);
                let _ = self
                    .send_alert(
                        "validate_block_consensus_failed",
                        &format!("Consensus validation failed: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Consensus validation failed: {}", e))
            })?
            .into_inner();

        if !consensus_response.success {
            warn!("Block validation failed: {}", consensus_response.error);
            let _ = self
                .send_alert(
                    "validate_block_failed",
                    &format!("Block validation failed: {}", consensus_response.error),
                    2,
                )
                .await;
            return Ok(Response::new(ValidateBlockResponse {
                success: false,
                error: consensus_response.error,
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn assemble_block(
        &self,
        request: Request<AssembleBlockRequest>,
    ) -> Result<Response<AssembleBlockResponse>, Status> {
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
        self.authorize(&user_id, "AssembleBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut block = Block::new();
        let mut total_size = 0;

        for tx_hex in req.tx_hexes {
            let tx_bytes = hex::decode(&tx_hex)
                .map_err(|e| {
                    warn!("Invalid transaction hex: {}", e);
                    let _ = self
                        .send_alert(
                            "assemble_invalid_tx",
                            &format!("Invalid transaction hex: {}", e),
                            2,
                        )
                        .await;
                    Status::invalid_argument(format!("Invalid transaction hex: {}", e))
                })?;
            let tx: sv::transaction::Transaction = deserialize(&tx_bytes)
                .map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    let _ = self
                        .send_alert(
                            "assemble_invalid_tx_deserialization",
                            &format!("Invalid transaction: {}", e),
                            2,
                        )
                        .await;
                    Status::invalid_argument(format!("Invalid transaction: {}", e))
                })?;
            total_size += tx.serialized_size();
            if total_size > 34_359_738_368 {
                warn!("Block size exceeds 32GB: {}", total_size);
                let _ = self
                    .send_alert(
                        "assemble_block_size_exceeded",
                        &format!("Block size exceeds 32GB: {}", total_size),
                        3,
                    )
                    .await;
                return Ok(Response::new(AssembleBlockResponse {
                    success: false,
                    block_hex: "".to_string(),
                    error: "Block size exceeds 32GB".to_string(),
                }));
            }
            block.add_transaction(tx);
        }

        let block_hex = hex::encode(serialize(&block));
        let mut blocks = self.blocks.lock().await;
        blocks.push(block_hex.clone());

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(AssembleBlockResponse {
            success: true,
            block_hex,
            error: "".to_string(),
        }))
    }

    async fn index_block(
        &self,
        request: Request<IndexBlockRequest>,
    ) -> Result<Response<IndexBlockResponse>, Status> {
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
        self.authorize(&user_id, "IndexBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| {
                warn!("Invalid block hex: {}", e);
                let _ = self
                    .send_alert(
                        "index_invalid_block",
                        &format!("Invalid block hex: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid block hex: {}", e))
            })?;
        let _block: Block = deserialize(&block_bytes)
            .map_err(|e| {
                warn!("Invalid block: {}", e);
                let _ = self
                    .send_alert(
                        "index_invalid_block_deserialization",
                        &format!("Invalid block: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid block: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(IndexBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_block_headers(
        &self,
        request: Request<GetBlockHeadersRequest>,
    ) -> Result<Response<GetBlockHeadersResponse>, Status> {
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
        self.authorize(&user_id, "GetBlockHeaders").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let blocks = self.blocks.lock().await;
        let headers = blocks
            .iter()
            .filter(|block_hex| block_hex.contains(&req.block_hash))
            .map(|block_hex| block_hex.clone())
            .collect();

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(GetBlockHeadersResponse {
            headers,
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
            service_name: "block_service".to_string(),
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

    let addr = "[::1]:50054".parse().unwrap();
    let block_service = BlockServiceImpl::new().await;

    println!("Block service listening on {}", addr);

    Server::builder()
        .add_service(BlockServer::new(block_service))
        .serve(addr)
        .await?;

    Ok(())
            }
