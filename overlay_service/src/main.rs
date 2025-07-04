use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use async_stream::try_stream;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use governor::{Quota, RateLimiter};
use overlay::overlay_server::{Overlay, OverlayServer};
use overlay::{
    AssembleOverlayBlockRequest, AssembleOverlayBlockResponse, CreateOverlayRequest,
    CreateOverlayResponse, GetMetricsRequest, GetMetricsResponse, JoinOverlayRequest,
    JoinOverlayResponse, LeaveOverlayRequest, LeaveOverlayResponse, StreamOverlayDataRequest,
    StreamOverlayDataResponse, ValidateOverlayConsensusRequest, ValidateOverlayConsensusResponse,
};
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use sled::Db;
use std::num::NonZeroU32;
use std::sync::Arc;
use sv::block::Block;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use tokio::sync::Mutex;
use tokio_stream::Stream;
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status, Streaming,
};
use tracing::{info, warn};

tonic::include_proto!("overlay");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct OverlayServiceImpl {
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    db: Arc<Mutex<Db>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    rate_limiter:
        Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl OverlayServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let db = Arc::new(Mutex::new(
            sled::open("overlay_db").expect("Failed to open sled database"),
        ));
        let registry = Arc::new(Registry::new());
        let requests_total =
            Counter::new("overlay_requests_total", "Total overlay requests").unwrap();
        let latency_ms =
            Gauge::new("overlay_latency_ms", "Average overlay request latency").unwrap();
        let alert_count = Counter::new("overlay_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        OverlayServiceImpl {
            auth_client,
            alert_client,
            db,
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
            service: "overlay_service".to_string(),
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

    async fn validate_overlay_consensus(&self, block: &Block) -> Result<(), String> {
        if block.serialized_size() > 34_359_738_368 {
            return Err(format!(
                "Block size exceeds 32GB: {}",
                block.serialized_size()
            ));
        }
        for tx in &block.transactions {
            for output in &tx.output {
                if output.script.is_op_return() && output.script.serialized_size() > 4_294_967_296 {
                    return Err(format!(
                        "OP_RETURN size exceeds 4.3GB: {}",
                        output.script.serialized_size()
                    ));
                }
            }
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl Overlay for OverlayServiceImpl {
    async fn create_overlay(
        &self,
        request: Request<CreateOverlayRequest>,
    ) -> Result<Response<CreateOverlayResponse>, Status> {
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
        self.authorize(&user_id, "CreateOverlay").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let db = self.db.lock().await;
        let key = format!("overlay:{}", req.overlay_id);
        db.insert(key.as_bytes(), req.consensus_rules.as_bytes())
            .map_err(|e| {
                warn!("Failed to create overlay: {}", e);
                let _ = self
                    .send_alert(
                        "create_overlay_failed",
                        &format!("Failed to create overlay: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to create overlay: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(CreateOverlayResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn join_overlay(
        &self,
        request: Request<JoinOverlayRequest>,
    ) -> Result<Response<JoinOverlayResponse>, Status> {
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
        self.authorize(&user_id, "JoinOverlay").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let db = self.db.lock().await;
        let key = format!("overlay:{}", req.overlay_id);

        if db
            .get(key.as_bytes())
            .map_err(|e| {
                warn!("Failed to access overlay: {}", e);
                let _ = self
                    .send_alert(
                        "join_overlay_failed",
                        &format!("Failed to access overlay: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to access overlay: {}", e))
            })?
            .is_none()
        {
            warn!("Overlay not found: {}", req.overlay_id);
            let _ = self
                .send_alert(
                    "join_overlay_not_found",
                    &format!("Overlay not found: {}", req.overlay_id),
                    2,
                )
                .await;
            return Ok(Response::new(JoinOverlayResponse {
                success: false,
                error: "Overlay not found".to_string(),
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(JoinOverlayResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn leave_overlay(
        &self,
        request: Request<LeaveOverlayRequest>,
    ) -> Result<Response<LeaveOverlayResponse>, Status> {
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
        self.authorize(&user_id, "LeaveOverlay").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let db = self.db.lock().await;
        let key = format!("overlay:{}", req.overlay_id);

        if db
            .get(key.as_bytes())
            .map_err(|e| {
                warn!("Failed to access overlay: {}", e);
                let _ = self
                    .send_alert(
                        "leave_overlay_failed",
                        &format!("Failed to access overlay: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to access overlay: {}", e))
            })?
            .is_none()
        {
            warn!("Overlay not found: {}", req.overlay_id);
            let _ = self
                .send_alert(
                    "leave_overlay_not_found",
                    &format!("Overlay not found: {}", req.overlay_id),
                    2,
                )
                .await;
            return Ok(Response::new(LeaveOverlayResponse {
                success: false,
                error: "Overlay not found".to_string(),
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(LeaveOverlayResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    type StreamOverlayDataStream =
        Pin<Box<dyn Stream<Item = Result<StreamOverlayDataResponse, Status>> + Send>>;

    async fn stream_overlay_data(
        &self,
        request: Request<Streaming<StreamOverlayDataRequest>>,
    ) -> Result<Response<Self::StreamOverlayDataStream>, Status> {
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
        self.authorize(&user_id, "StreamOverlayData").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let mut stream = request.into_inner();
        let db = self.db.clone();
        let alert_client = self.alert_client.clone();
        let requests_total = self.requests_total.clone();
        let latency_ms = self.latency_ms.clone();

        let output = try_stream! {
            while let Some(req) = stream.next().await {
                requests_total.inc();
                let start_inner = std::time::Instant::now();
                let db = db.lock().await;
                let key = format!("overlay:{}", req.overlay_id);
                match db.get(key.as_bytes()) {
                    Ok(Some(value)) => {
                        yield StreamOverlayDataResponse {
                            success: true,
                            data: value.to_vec(),
                            error: "".to_string(),
                        };
                    }
                    Ok(None) => {
                        warn!("Overlay not found: {}", req.overlay_id);
                        let _ = alert_client
                            .clone()
                            .send_alert(SendAlertRequest {
                                event_type: "stream_overlay_not_found".to_string(),
                                message: format!("Overlay not found: {}", req.overlay_id),
                                severity: 2,
                            })
                            .await;
                        yield StreamOverlayDataResponse {
                            success: false,
                            data: vec![],
                            error: "Overlay not found".to_string(),
                        };
                    }
                    Err(e) => {
                        warn!("Failed to access overlay: {}", e);
                        let _ = alert_client
                            .clone()
                            .send_alert(SendAlertRequest {
                                event_type: "stream_overlay_failed".to_string(),
                                message: format!("Failed to access overlay: {}", e),
                                severity: 2,
                            })
                            .await;
                        yield StreamOverlayDataResponse {
                            success: false,
                            data: vec![],
                            error: format!("Failed to access overlay: {}", e),
                        };
                    }
                }
                latency_ms.set(start_inner.elapsed().as_secs_f64() * 1000.0);
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(output)))
    }

    async fn validate_overlay_consensus(
        &self,
        request: Request<ValidateOverlayConsensusRequest>,
    ) -> Result<Response<ValidateOverlayConsensusResponse>, Status> {
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
        self.authorize(&user_id, "ValidateOverlayConsensus").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex).map_err(|e| {
            warn!("Invalid block hex: {}", e);
            let _ = self
                .send_alert(
                    "validate_overlay_invalid_hex",
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
                    "validate_overlay_invalid_deserialization",
                    &format!("Invalid block: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid block: {}", e))
        })?;

        let result = self.validate_overlay_consensus(&block).await.map_err(|e| {
            warn!("Overlay validation failed: {}", e);
            let _ = self
                .send_alert(
                    "validate_overlay_failed",
                    &format!("Overlay validation failed: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(e)
        })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateOverlayConsensusResponse {
            success: result.is_ok(),
            error: result.err().unwrap_or_default(),
        }))
    }

    async fn assemble_overlay_block(
        &self,
        request: Request<AssembleOverlayBlockRequest>,
    ) -> Result<Response<AssembleOverlayBlockResponse>, Status> {
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
        self.authorize(&user_id, "AssembleOverlayBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut block = Block::new();
        let mut total_size = 0;

        for tx_hex in req.tx_hexes {
            let tx_bytes = hex::decode(&tx_hex).map_err(|e| {
                warn!("Invalid transaction hex: {}", e);
                let _ = self
                    .send_alert(
                        "assemble_overlay_invalid_tx",
                        &format!("Invalid transaction hex: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid transaction hex: {}", e))
            })?;
            let tx: Transaction = deserialize(&tx_bytes).map_err(|e| {
                warn!("Invalid transaction: {}", e);
                let _ = self
                    .send_alert(
                        "assemble_overlay_invalid_tx_deserialization",
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
                        "assemble_overlay_block_size_exceeded",
                        &format!("Block size exceeds 32GB: {}", total_size),
                        3,
                    )
                    .await;
                return Ok(Response::new(AssembleOverlayBlockResponse {
                    success: false,
                    block_hex: "".to_string(),
                    error: "Block size exceeds 32GB".to_string(),
                }));
            }
            block.add_transaction(tx);
        }

        let block_hex = hex::encode(serialize(&block));
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(AssembleOverlayBlockResponse {
            success: true,
            block_hex,
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
            service_name: "overlay_service".to_string(),
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

    let addr = "[::1]:50056".parse().unwrap();
    let overlay_service = OverlayServiceImpl::new().await;

    println!("Overlay service listening on {}", addr);

    Server::builder()
        .add_service(OverlayServer::new(overlay_service))
        .serve(addr)
        .await?;

    Ok(())
}
