use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use async_channel::Sender;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateTransactionConsensusRequest;
use governor::{Quota, RateLimiter};
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use std::num::NonZeroU32;
use std::sync::Arc;
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use tokio::sync::Mutex;
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};
use tracing::{info, warn};
use transaction::transaction_server::{Transaction, TransactionServer};
use transaction::{
    BatchValidateTransactionRequest, BatchValidateTransactionResponse, GetMetricsRequest,
    GetMetricsResponse, IndexTransactionRequest, IndexTransactionResponse,
    ProcessTransactionRequest, ProcessTransactionResponse, ValidateTransactionRequest,
    ValidateTransactionResponse,
};

tonic::include_proto!("transaction");
tonic::include_proto!("storage");
tonic::include_proto!("consensus");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct TransactionServiceImpl {
    storage_client: StorageClient<Channel>,
    consensus_client: ConsensusClient<Channel>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    tx_queue: Sender<String>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    index_throughput: Gauge,
    rate_limiter:
        Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl TransactionServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        let consensus_client = ConsensusClient::connect("http://[::1]:50055")
            .await
            .expect("Failed to connect to consensus_service");
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let (tx_queue, _) = async_channel::unbounded();
        let registry = Arc::new(Registry::new());
        let requests_total =
            Counter::new("transaction_requests_total", "Total transaction requests").unwrap();
        let latency_ms = Gauge::new(
            "transaction_latency_ms",
            "Average transaction processing latency",
        )
        .unwrap();
        let alert_count = Counter::new("transaction_alert_count", "Total alerts sent").unwrap();
        let index_throughput = Gauge::new(
            "transaction_index_throughput",
            "Indexed transactions per second",
        )
        .unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry
            .register(Box::new(index_throughput.clone()))
            .unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        TransactionServiceImpl {
            storage_client,
            consensus_client,
            auth_client,
            alert_client,
            tx_queue,
            registry,
            requests_total,
            latency_ms,
            alert_count,
            index_throughput,
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
            service: "transaction_service".to_string(),
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
impl Transaction for TransactionServiceImpl {
    async fn validate_transaction(
        &self,
        request: Request<ValidateTransactionRequest>,
    ) -> Result<Response<ValidateTransactionResponse>, Status> {
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
        self.authorize(&user_id, "ValidateTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex).map_err(|e| {
            warn!("Invalid transaction hex: {}", e);
            let _ = self
                .send_alert(
                    "validate_invalid_tx",
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
                    "validate_invalid_tx_deserialization",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        let consensus_request = ValidateTransactionConsensusRequest {
            tx_hex: req.tx_hex.clone(),
        };
        let consensus_response = self
            .consensus_client
            .validate_transaction_consensus(consensus_request)
            .await
            .map_err(|e| {
                warn!("Consensus validation failed: {}", e);
                let _ = self
                    .send_alert(
                        "validate_consensus_failed",
                        &format!("Consensus validation failed: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Consensus validation failed: {}", e))
            })?
            .into_inner();

        if !consensus_response.success {
            warn!(
                "Transaction validation failed: {}",
                consensus_response.error
            );
            let _ = self
                .send_alert(
                    "validate_tx_failed",
                    &format!(
                        "Transaction validation failed: {}",
                        consensus_response.error
                    ),
                    2,
                )
                .await;
            return Ok(Response::new(ValidateTransactionResponse {
                success: false,
                error: consensus_response.error,
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn process_transaction(
        &self,
        request: Request<ProcessTransactionRequest>,
    ) -> Result<Response<ProcessTransactionResponse>, Status> {
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
        self.authorize(&user_id, "ProcessTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex).map_err(|e| {
            warn!("Invalid transaction hex: {}", e);
            let _ = self
                .send_alert(
                    "process_invalid_tx",
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
                    "process_invalid_tx_deserialization",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        let storage_request = QueryUtxoRequest {
            txid: tx.txid().to_string(),
            vout: 0,
        };
        let storage_response = self
            .storage_client
            .query_utxo(storage_request)
            .await
            .map_err(|e| {
                warn!("UTXO query failed: {}", e);
                let _ = self
                    .send_alert(
                        "process_utxo_query_failed",
                        &format!("UTXO query failed: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("UTXO query failed: {}", e))
            })?
            .into_inner();

        if !storage_response.exists {
            warn!("UTXO not found for txid: {}", tx.txid());
            let _ = self
                .send_alert(
                    "process_utxo_not_found",
                    &format!("UTXO not found for txid: {}", tx.txid()),
                    3,
                )
                .await;
            return Ok(Response::new(ProcessTransactionResponse {
                success: false,
                error: "UTXO not found".to_string(),
            }));
        }

        self.tx_queue.send(req.tx_hex.clone()).await.map_err(|e| {
            warn!("Failed to queue transaction: {}", e);
            let _ = self
                .send_alert(
                    "process_queue_failed",
                    &format!("Failed to queue transaction: {}", e),
                    2,
                )
                .await;
            Status::internal(format!("Failed to queue transaction: {}", e))
        })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ProcessTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn batch_validate_transaction(
        &self,
        request: Request<BatchValidateTransactionRequest>,
    ) -> Result<Response<BatchValidateTransactionResponse>, Status> {
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
        self.authorize(&user_id, "BatchValidateTransaction").await?;
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
                        "batch_validate_invalid_tx",
                        &format!("Invalid transaction hex: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid transaction hex: {}", e))
            })?;
            let _tx: Transaction = deserialize(&tx_bytes).map_err(|e| {
                warn!("Invalid transaction: {}", e);
                let _ = self
                    .send_alert(
                        "batch_validate_invalid_tx_deserialization",
                        &format!("Invalid transaction: {}", e),
                        2,
                    )
                    .await;
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

            let consensus_request = ValidateTransactionConsensusRequest {
                tx_hex: tx_hex.clone(),
            };
            let consensus_response = self
                .consensus_client
                .validate_transaction_consensus(consensus_request)
                .await
                .map_err(|e| {
                    warn!("Consensus validation failed: {}", e);
                    let _ = self
                        .send_alert(
                            "batch_validate_consensus_failed",
                            &format!("Consensus validation failed: {}", e),
                            2,
                        )
                        .await;
                    Status::internal(format!("Consensus validation failed: {}", e))
                })?
                .into_inner();

            results.push(ValidateTransactionResponse {
                success: consensus_response.success,
                error: consensus_response.error,
            });
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchValidateTransactionResponse { results }))
    }

    async fn index_transaction(
        &self,
        request: Request<IndexTransactionRequest>,
    ) -> Result<Response<IndexTransactionResponse>, Status> {
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
        self.authorize(&user_id, "IndexTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex).map_err(|e| {
            warn!("Invalid transaction hex: {}", e);
            let _ = self
                .send_alert(
                    "index_invalid_tx",
                    &format!("Invalid transaction hex: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction hex: {}", e))
        })?;
        let _tx: Transaction = deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self
                .send_alert(
                    "index_invalid_tx_deserialization",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        self.index_throughput.set(1.0); // Placeholder
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(IndexTransactionResponse {
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
            service_name: "transaction_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: 0,   // Not applicable
            alert_count: self.alert_count.get() as u64,
            index_throughput: self.index_throughput.get(),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "[::1]:50052".parse().unwrap();
    let transaction_service = TransactionServiceImpl::new().await;

    println!("Transaction service listening on {}", addr);

    Server::builder()
        .add_service(TransactionServer::new(transaction_service))
        .serve(addr)
        .await?;

    Ok(())
}
