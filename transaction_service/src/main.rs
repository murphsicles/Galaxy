use tonic::{transport::{Server, Channel}, Request, Response, Status};
use transaction::transaction_server::{Transaction, TransactionServer};
use transaction::{ValidateTxRequest, ValidateTxResponse, ProcessTxRequest, ProcessTxResponse, BatchValidateTxRequest, BatchValidateTxResponse, IndexTransactionRequest, IndexTransactionResponse};
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateTxConsensusRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use index::index_client::IndexClient;
use index::IndexTransactionRequest as IndexIndexTransactionRequest;
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::transaction::{Transaction, Input};
use sv::script::Script;
use sv::util::{deserialize, serialize};
use async_channel::{Sender, Receiver};
use hex;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter, Jitter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use toml;

tonic::include_proto!("transaction");
tonic::include_proto!("storage");
tonic::include_proto!("consensus");
tonic::include_proto!("auth");
tonic::include_proto!("index");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct TransactionServiceImpl {
    storage_client: StorageClient<Channel>,
    consensus_client: ConsensusClient<Channel>,
    auth_client: AuthClient<Channel>,
    index_client: IndexClient<Channel>,
    alert_client: AlertClient<Channel>,
    tx_queue: Arc<Mutex<(Sender<String>, Receiver<String>)>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    index_throughput: Gauge,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
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
        let index_client = IndexClient::connect("http://[::1]:50062")
            .await
            .expect("Failed to connect to index_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let (tx, rx) = async_channel::bounded(1000);
        let tx_queue = Arc::new(Mutex::new((tx, rx)));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("transaction_requests_total", "Total transaction requests").unwrap();
        let latency_ms = Gauge::new("transaction_latency_ms", "Average transaction processing latency").unwrap();
        let alert_count = Counter::new("transaction_alert_count", "Total alerts sent").unwrap();
        let index_throughput = Gauge::new("transaction_index_throughput", "Indexed transactions per second").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(index_throughput.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));
        let shard_manager = Arc::new(ShardManager::new());

        let tx_queue_clone = Arc::clone(&tx_queue);
        tokio::spawn(async move {
            let mut queue = tx_queue_clone.lock().await;
            while let Ok(tx_hex) = queue.1.recv().await {
                info!("Processing queued transaction: {}", tx_hex);
            }
        });

        TransactionServiceImpl {
            storage_client,
            consensus_client,
            auth_client,
            index_client,
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
            service: "transaction_service".to_string(),
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

    async fn validate_inputs(&self, tx: &Transaction) -> Result<bool, String> {
        let mut client = self.storage_client.clone();
        for input in &tx.inputs {
            let request = QueryUtxoRequest {
                txid: input.previous_output.txid.to_string(),
                vout: input.previous_output.vout,
            };
            let response = client
                .query_utxo(request)
                .await
                .map_err(|e| format!("UTXO query failed: {}", e))?
                .into_inner();

            if !response.exists {
                let error = format!("UTXO not found: {}:{}", input.previous_output.txid, input.previous_output.vout);
                let _ = self.send_alert("utxo_not_found", &error, 2).await;
                return Err(error);
            }

            let script_pubkey: Script = deserialize(&hex::decode(&response.script_pubkey)
                .map_err(|e| format!("Invalid script_pubkey: {}", e))?)?;
            if !script_pubkey.is_standard() {
                let error = "Non-standard script".to_string();
                let _ = self.send_alert("non_standard_script", &error, 2).await;
                return Err(error);
            }
        }
        Ok(true)
    }
}

#[tonic::async_trait]
impl Transaction for TransactionServiceImpl {
    async fn validate_transaction(&self, request: Request<ValidateTxRequest>) -> Result<Response<ValidateTxResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "ValidateTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Validating transaction: {}", request.get_ref().tx_hex);
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| {
                warn!("Invalid tx_hex: {}", e);
                let _ = self.send_alert("tx_invalid_format", &format!("Invalid tx_hex: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid tx_hex: {}", e))
            })?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| {
                warn!("Invalid transaction: {}", e);
                let _ = self.send_alert("tx_invalid_format", &format!("Invalid transaction: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Invalid transaction format: empty inputs or outputs");
            let _ = self.send_alert("tx_invalid_format", "Invalid transaction format: empty inputs or outputs", 2).await;
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: "Invalid transaction format: empty inputs or outputs".to_string(),
            }));
        }

        let mut consensus_client = self.consensus_client.clone();
        let consensus_request = ValidateTxConsensusRequest { tx_hex: req.tx_hex.clone() };
        let consensus_response = consensus_client
            .validate_transaction_consensus(consensus_request)
            .await
            .map_err(|e| {
                warn!("Consensus validation failed: {}", e);
                let _ = self.send_alert("tx_consensus_failed", &format!("Consensus validation failed: {}", e), 2).await;
                Status::internal(format!("Consensus validation failed: {}", e))
            })?
            .into_inner();
        if !consensus_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Consensus validation failed: {}", consensus_response.error);
            let _ = self.send_alert("tx_consensus_failed", &format!("Consensus validation failed: {}", consensus_response.error), 2).await;
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: consensus_response.error,
            }));
        }

        let is_valid = match self.validate_inputs(&tx).await {
            Ok(_) => true,
            Err(e) => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                warn!("Input validation failed: {}", e);
                return Ok(Response::new(ValidateTxResponse {
                    is_valid: false,
                    error: e,
                }));
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Transaction validation {}: {}", if is_valid { "succeeded" } else { "failed" }, req.tx_hex);
        Ok(Response::new(ValidateTxResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Input validation failed".to_string() },
        }))
    }

    async fn process_transaction(&self, request: Request<ProcessTxRequest>) -> Result<Response<ProcessTxResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "ProcessTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Processing transaction: {}", request.get_ref().tx_hex);
        let req = request.into_inner();
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
        let validate_response = self
            .validate_transaction(Request::new(validate_request))
            .await?
            .into_inner();

        if !validate_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Transaction validation failed: {}", validate_response.error);
            let _ = self.send_alert("tx_validation_failed", &format!("Transaction validation failed: {}", validate_response.error), 2).await;
            return Ok(Response::new(ProcessTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let mut index_client = self.index_client.clone();
        let index_request = IndexIndexTransactionRequest { tx_hex: req.tx_hex.clone() };
        let index_response = index_client
            .index_transaction(index_request)
            .await
            .map_err(|e| {
                warn!("Transaction indexing failed: {}", e);
                let _ = self.send_alert("tx_indexing_failed", &format!("Transaction indexing failed: {}", e), 2).await;
                Status::internal(format!("Transaction indexing failed: {}", e))
            })?
            .into_inner();
        if !index_response.success {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Transaction indexing failed: {}", index_response.error);
            let _ = self.send_alert("tx_indexing_failed", &format!("Transaction indexing failed: {}", index_response.error), 2).await;
            return Ok(Response::new(ProcessTxResponse {
                success: false,
                error: index_response.error,
            }));
        }
        self.index_throughput.inc_by(1.0);

        let tx_queue = self.tx_queue.lock().await;
        tx_queue
            .0
            .send(req.tx_hex)
            .await
            .map_err(|e| {
                warn!("Failed to queue transaction: {}", e);
                let _ = self.send_alert("tx_queue_failed", &format!("Failed to queue transaction: {}", e), 2).await;
                Status::internal(format!("Failed to queue transaction: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully processed transaction: {}", req.tx_hex);
        Ok(Response::new(ProcessTxResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn batch_validate_transaction(&self, request: Request<BatchValidateTxRequest>) -> Result<Response<BatchValidateTxResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BatchValidateTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Batch validating {} transactions", request.get_ref().tx_hexes.len());
        let req = request.into_inner();
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxRequest { tx_hex };
            let result = self
                .validate_transaction(Request::new(validate_request))
                .await?
                .into_inner();
            results.push(result);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Completed batch validation for {} transactions", results.len());
        Ok(Response::new(BatchValidateTxResponse { results }))
    }

    async fn index_transaction(&self, request: Request<IndexTransactionRequest>) -> Result<Response<IndexTransactionResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "IndexTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Indexing transaction: {}", request.get_ref().tx_hex);
        let req = request.into_inner();
        let mut index_client = self.index_client.clone();
        let index_request = IndexIndexTransactionRequest { tx_hex: req.tx_hex.clone() };
        let index_response = index_client
            .index_transaction(index_request)
            .await
            .map_err(|e| {
                warn!("Transaction indexing failed: {}", e);
                let _ = self.send_alert("tx_indexing_failed", &format!("Transaction indexing failed: {}", e), 2).await;
                Status::internal(format!("Transaction indexing failed: {}", e))
            })?
            .into_inner();

        self.index_throughput.inc_by(1.0);
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Transaction indexing {}: {}", if index_response.success { "succeeded" } else { "failed" }, req.tx_hex);
        Ok(Response::new(IndexTransactionResponse {
            success: index_response.success,
            error: index_response.error,
        }))
    }

    async fn get_metrics(&self, request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetMetrics").await?;

        Ok(Response::new(GetMetricsResponse {
            service_name: "transaction_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: 0, // Not applicable
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
