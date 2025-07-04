use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use consensus::consensus_server::{Consensus, ConsensusServer};
use consensus::{
    BatchValidateTransactionConsensusRequest, BatchValidateTransactionConsensusResponse,
    GetMetricsRequest, GetMetricsResponse, ValidateBlockConsensusRequest,
    ValidateBlockConsensusResponse, ValidateTransactionConsensusRequest,
    ValidateTransactionConsensusResponse,
};
use governor::{Quota, RateLimiter};
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use std::num::NonZeroU32;
use std::sync::Arc;
use sv::block::Block;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use tokio::sync::Mutex;
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};
use tracing::{info, warn};

tonic::include_proto!("consensus");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct ConsensusServiceImpl {
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

impl ConsensusServiceImpl {
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
        let registry = Arc::new(Registry::new());
        let requests_total =
            Counter::new("consensus_requests_total", "Total consensus requests").unwrap();
        let latency_ms =
            Gauge::new("consensus_latency_ms", "Average consensus request latency").unwrap();
        let alert_count = Counter::new("consensus_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        ConsensusServiceImpl {
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
            service: "consensus_service".to_string(),
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

    async fn validate_block_rules(&self, block: &Block) -> Result<(), String> {
        if block.serialized_size() > 34_359_738_368 {
            return Err(format!(
                "Block size exceeds 32GB: {}",
                block.serialized_size()
            ));
        }
        Ok(())
    }

    async fn validate_transaction_rules(&self, tx: &Transaction) -> Result<(), String> {
        if tx.serialized_size() > 34_359_738_368 {
            return Err(format!(
                "Transaction size exceeds 32GB: {}",
                tx.serialized_size()
            ));
        }
        for output in &tx.output {
            if output.script.is_op_return() && output.script.serialized_size() > 4_294_967_296 {
                return Err(format!(
                    "OP_RETURN size exceeds 4.3GB: {}",
                    output.script.serialized_size()
                ));
            }
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl Consensus for ConsensusServiceImpl {
    async fn validate_block_consensus(
        &self,
        request: Request<ValidateBlockConsensusRequest>,
    ) -> Result<Response<ValidateBlockConsensusResponse>, Status> {
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
        self.authorize(&user_id, "ValidateBlockConsensus").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex).map_err(|e| {
            warn!("Invalid block hex: {}", e);
            let _ = self
                .send_alert(
                    "validate_block_invalid_hex",
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
                    "validate_block_invalid_deserialization",
                    &format!("Invalid block: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid block: {}", e))
        })?;

        let result = self.validate_block_rules(&block).await.map_err(|e| {
            warn!("Block validation failed: {}", e);
            let _ = self
                .send_alert(
                    "validate_block_failed",
                    &format!("Block validation failed: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(e)
        })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateBlockConsensusResponse {
            success: result.is_ok(),
            error: result.err().unwrap_or_default(),
        }))
    }

    async fn validate_transaction_consensus(
        &self,
        request: Request<ValidateTransactionConsensusRequest>,
    ) -> Result<Response<ValidateTransactionConsensusResponse>, Status> {
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
        self.authorize(&user_id, "ValidateTransactionConsensus")
            .await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex).map_err(|e| {
            warn!("Invalid transaction hex: {}", e);
            let _ = self
                .send_alert(
                    "validate_tx_invalid_hex",
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
                    "validate_tx_invalid_deserialization",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        let result = self.validate_transaction_rules(&tx).await.map_err(|e| {
            warn!("Transaction validation failed: {}", e);
            let _ = self
                .send_alert(
                    "validate_tx_failed",
                    &format!("Transaction validation failed: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(e)
        })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateTransactionConsensusResponse {
            success: result.is_ok(),
            error: result.err().unwrap_or_default(),
        }))
    }

    async fn batch_validate_transaction_consensus(
        &self,
        request: Request<BatchValidateTransactionConsensusRequest>,
    ) -> Result<Response<BatchValidateTransactionConsensusResponse>, Status> {
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
        self.authorize(&user_id, "BatchValidateTransactionConsensus")
            .await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let tx_bytes = hex::decode(&tx_hex).map_err(|e| {
                warn!("Invalid transaction hex: {}", e);
                let _ = self
                    .send_alert(
                        "batch_validate_tx_invalid_hex",
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
                        "batch_validate_tx_invalid_deserialization",
                        &format!("Invalid transaction: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

            let result = self.validate_transaction_rules(&tx).await.map_err(|e| {
                warn!("Transaction validation failed: {}", e);
                let _ = self
                    .send_alert(
                        "batch_validate_tx_failed",
                        &format!("Transaction validation failed: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(e)
            })?;

            results.push(ValidateTransactionConsensusResponse {
                success: result.is_ok(),
                error: result.err().unwrap_or_default(),
            });
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchValidateTransactionConsensusResponse {
            results,
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
            service_name: "consensus_service".to_string(),
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

    let addr = "[::1]:50055".parse().unwrap();
    let consensus_service = ConsensusServiceImpl::new().await;

    println!("Consensus service listening on {}", addr);

    Server::builder()
        .add_service(ConsensusServer::new(consensus_service))
        .serve(addr)
        .await?;

    Ok(())
}
