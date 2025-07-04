use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use async_stream::try_stream;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use governor::{Quota, RateLimiter};
use mining::mining_server::{Mining, MiningServer};
use mining::{
    GenerateBlockTemplateRequest, GenerateBlockTemplateResponse, GetMetricsRequest,
    GetMetricsResponse, StreamMiningWorkRequest, StreamMiningWorkResponse, SubmitMinedBlockRequest,
    SubmitMinedBlockResponse,
};
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use std::num::NonZeroU32;
use std::sync::Arc;
use sv::block::Block;
use sv::util::{deserialize, serialize};
use tokio::sync::Mutex;
use tokio_stream::Stream;
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status, Streaming,
};
use tracing::{info, warn};

tonic::include_proto!("mining");
tonic::include_proto!("block");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct MiningServiceImpl {
    block_client: BlockClient<Channel>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    rate_limiter:
        Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl MiningServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

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
        let requests_total =
            Counter::new("mining_requests_total", "Total mining requests").unwrap();
        let latency_ms = Gauge::new("mining_latency_ms", "Average mining request latency").unwrap();
        let alert_count = Counter::new("mining_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        MiningServiceImpl {
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
            service: "mining_service".to_string(),
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

    async fn send_alert(
        &self,
        event_type: &str,
        message: &str,
        severity: u32,
    ) -> Result<(), Status> {
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
impl Mining for MiningServiceImpl {
    async fn generate_block_template(
        &self,
        request: Request<GenerateBlockTemplateRequest>,
    ) -> Result<Response<GenerateBlockTemplateResponse>, Status> {
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
        self.authorize(&user_id, "GenerateBlockTemplate").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_request = AssembleBlockRequest {
            tx_hexes: req.tx_hexes,
        };
        let block_response = self
            .block_client
            .assemble_block(block_request)
            .await
            .map_err(|e| {
                warn!("Failed to assemble block: {}", e);
                let _ = self
                    .send_alert(
                        "generate_block_template_failed",
                        &format!("Failed to assemble block: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to assemble block: {}", e))
            })?
            .into_inner();

        if !block_response.success {
            warn!("Block assembly failed: {}", block_response.error);
            let _ = self
                .send_alert(
                    "generate_block_template_failed",
                    &format!("Block assembly failed: {}", block_response.error),
                    2,
                )
                .await;
            return Ok(Response::new(GenerateBlockTemplateResponse {
                success: false,
                block_hex: "".to_string(),
                error: block_response.error,
            }));
        }

        if block_response.block_hex.len() > 34_359_738_368 {
            warn!(
                "Block size exceeds 32GB: {}",
                block_response.block_hex.len()
            );
            let _ = self
                .send_alert(
                    "generate_block_template_size_exceeded",
                    &format!(
                        "Block size exceeds 32GB: {}",
                        block_response.block_hex.len()
                    ),
                    3,
                )
                .await;
            return Ok(Response::new(GenerateBlockTemplateResponse {
                success: false,
                block_hex: "".to_string(),
                error: "Block size exceeds 32GB".to_string(),
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(GenerateBlockTemplateResponse {
            success: true,
            block_hex: block_response.block_hex,
            error: "".to_string(),
        }))
    }

    async fn submit_mined_block(
        &self,
        request: Request<SubmitMinedBlockRequest>,
    ) -> Result<Response<SubmitMinedBlockResponse>, Status> {
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
        self.authorize(&user_id, "SubmitMinedBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex).map_err(|e| {
            warn!("Invalid block hex: {}", e);
            let _ = self
                .send_alert(
                    "submit_mined_block_invalid_hex",
                    &format!("Invalid block hex: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid block hex: {}", e))
        })?;
        let block: Block = deserialize(&block_bytes).map_err(|e| {
            warn!("Invalid block: {}", e);
            let _ = self
                .send_alert(
                    "submit_mined_block_invalid_deserialization",
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
                    "submit_mined_block_size_exceeded",
                    &format!("Block size exceeds 32GB: {}", block.serialized_size()),
                    3,
                )
                .await;
            return Ok(Response::new(SubmitMinedBlockResponse {
                success: false,
                error: "Block size exceeds 32GB".to_string(),
            }));
        }

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
                        "submit_mined_block_validation_failed",
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
                    "submit_mined_block_failed",
                    &format!("Block validation failed: {}", block_response.error),
                    2,
                )
                .await;
            return Ok(Response::new(SubmitMinedBlockResponse {
                success: false,
                error: block_response.error,
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(SubmitMinedBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    type StreamMiningWorkStream =
        Pin<Box<dyn Stream<Item = Result<StreamMiningWorkResponse, Status>> + Send>>;

    async fn stream_mining_work(
        &self,
        request: Request<Streaming<StreamMiningWorkRequest>>,
    ) -> Result<Response<Self::StreamMiningWorkStream>, Status> {
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
        self.authorize(&user_id, "StreamMiningWork").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let mut stream = request.into_inner();
        let block_client = self.block_client.clone();
        let alert_client = self.alert_client.clone();
        let requests_total = self.requests_total.clone();
        let latency_ms = self.latency_ms.clone();

        let output = try_stream! {
            while let Some(req) = stream.next().await {
                requests_total.inc();
                let start_inner = std::time::Instant::now();
                let block_request = AssembleBlockRequest {
                    tx_hexes: req.tx_hexes,
                };
                let block_response = block_client
                    .clone()
                    .assemble_block(block_request)
                    .await
                    .map_err(|e| {
                        warn!("Failed to assemble block: {}", e);
                        let _ = alert_client
                            .clone()
                            .send_alert(SendAlertRequest {
                                event_type: "stream_mining_work_failed".to_string(),
                                message: format!("Failed to assemble block: {}", e),
                                severity: 2,
                            })
                            .await;
                        Status::internal(format!("Failed to assemble block: {}", e))
                    })?
                    .into_inner();

                if !block_response.success {
                    warn!("Block assembly failed: {}", block_response.error);
                    let _ = alert_client
                        .clone()
                        .send_alert(SendAlertRequest {
                            event_type: "stream_mining_work_failed".to_string(),
                            message: format!("Block assembly failed: {}", block_response.error),
                            severity: 2,
                        })
                        .await;
                    yield StreamMiningWorkResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: block_response.error,
                    };
                    continue;
                }

                if block_response.block_hex.len() > 34_359_738_368 {
                    warn!("Block size exceeds 32GB: {}", block_response.block_hex.len());
                    let _ = alert_client
                        .clone()
                        .send_alert(SendAlertRequest {
                            event_type: "stream_mining_work_size_exceeded".to_string(),
                            message: format!("Block size exceeds 32GB: {}", block_response.block_hex.len()),
                            severity: 3,
                        })
                        .await;
                    yield StreamMiningWorkResponse {
                        success: false,
                        block_hex: "".to_string(),
                        error: "Block size exceeds 32GB".to_string(),
                    };
                    continue;
                }

                yield StreamMiningWorkResponse {
                    success: true,
                    block_hex: block_response.block_hex,
                    error: "".to_string(),
                };
                latency_ms.set(start_inner.elapsed().as_secs_f64() * 1000.0);
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(output)))
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
            service_name: "mining_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: 0,   // Not applicable
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

    let addr = "[::1]:50058".parse().unwrap();
    let mining_service = MiningServiceImpl::new().await;

    println!("Mining service listening on {}", addr);

    Server::builder()
        .add_service(MiningServer::new(mining_service))
        .serve(addr)
        .await?;

    Ok(())
}
