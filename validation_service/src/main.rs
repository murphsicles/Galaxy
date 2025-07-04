use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use block::block_client::BlockClient;
use block::GetBlockHeadersRequest;
use governor::{Jitter, Quota, RateLimiter};
use lru::LruCache;
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use std::num::NonZeroU32;
use std::sync::Arc;
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use sv::transaction::Transaction;
use sv::util::{deserialize, hash::Sha256d, serialize};
use tokio::sync::Mutex;
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status, Streaming,
};
use tracing::{info, warn};
use validation::validation_server::{Validation, ValidationServer};
use validation::{
    BatchGenerateSPVProofRequest, BatchGenerateSPVProofResponse, GenerateSPVProofRequest,
    GenerateSPVProofResponse, GetMetricsRequest, GetMetricsResponse, StreamSPVProofsRequest,
    StreamSPVProofsResponse, VerifySPVProofRequest, VerifySPVProofResponse,
};

tonic::include_proto!("validation");
tonic::include_proto!("block");
tonic::include_proto!("storage");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct ValidationServiceImpl {
    block_client: BlockClient<Channel>,
    storage_client: StorageClient<Channel>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    proof_cache: Arc<Mutex<LruCache<String, (String, Vec<String>)>>>,
    registry: Arc<Registry>,
    proof_requests: Counter,
    latency_ms: Gauge,
    cache_hits: Counter,
    alert_count: Counter,
    rate_limiter:
        Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl ValidationServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let proof_cache = Arc::new(Mutex::new(LruCache::new(NonZeroU32::new(1000).unwrap())));
        let registry = Arc::new(Registry::new());
        let proof_requests =
            Counter::new("validation_requests_total", "Total SPV proof requests").unwrap();
        let latency_ms =
            Gauge::new("validation_latency_ms", "Average proof generation latency").unwrap();
        let cache_hits = Counter::new("validation_cache_hits", "Total cache hits").unwrap();
        let alert_count = Counter::new("validation_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(proof_requests.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(cache_hits.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        ValidationServiceImpl {
            block_client,
            storage_client,
            auth_client,
            alert_client,
            proof_cache,
            registry,
            proof_requests,
            latency_ms,
            cache_hits,
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
            service: "validation_service".to_string(),
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

    async fn generate_spv_proof(&self, txid: &str) -> Result<(String, Vec<String>), Status> {
        self.proof_requests.inc();
        let start = std::time::Instant::now();

        let mut proof_cache = self.proof_cache.lock().await;
        if let Some(cached) = proof_cache.get(txid) {
            self.cache_hits.inc();
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(cached.clone());
        }

        let tx_bytes = hex::decode(txid).map_err(|e| {
            warn!("Invalid txid: {}", e);
            let _ = self
                .send_alert("spv_invalid_txid", &format!("Invalid txid: {}", e), 2)
                .await;
            Status::invalid_argument(format!("Invalid txid: {}", e))
        })?;
        let tx: Transaction = deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self
                .send_alert(
                    "spv_invalid_transaction",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        let block_request = GetBlockHeadersRequest {
            block_hash: tx.txid().to_string(),
        };
        let block_response = self
            .block_client
            .get_block_headers(block_request)
            .await
            .map_err(|e| {
                warn!("Failed to fetch block headers: {}", e);
                let _ = self
                    .send_alert(
                        "spv_block_fetch_failed",
                        &format!("Failed to fetch block headers: {}", e),
                        2,
                    )
                    .await;
                Status::internal(format!("Failed to fetch block headers: {}", e))
            })?
            .into_inner();

        if block_response.headers.is_empty() {
            warn!("No block headers found for txid: {}", txid);
            let _ = self
                .send_alert(
                    "spv_no_headers",
                    &format!("No block headers found for txid: {}", txid),
                    3,
                )
                .await;
            return Err(Status::not_found("No block headers found"));
        }

        let merkle_path = self.calculate_merkle_path(&tx).await.map_err(|e| {
            warn!("Failed to calculate merkle path: {}", e);
            let _ = self
                .send_alert(
                    "spv_merkle_path_failed",
                    &format!("Failed to calculate merkle path: {}", e),
                    2,
                )
                .await;
            Status::internal(format!("Failed to calculate merkle path: {}", e))
        })?;

        let merkle_path_hex = hex::encode(serialize(&merkle_path));
        proof_cache.put(
            txid.to_string(),
            (merkle_path_hex.clone(), block_response.headers.clone()),
        );
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);

        Ok((merkle_path_hex, block_response.headers))
    }

    async fn calculate_merkle_path(&self, tx: &Transaction) -> Result<Vec<Sha256d>, String> {
        info!("Calculating merkle path for txid: {}", tx.txid());
        Ok(vec![]) // Placeholder
    }

    async fn verify_spv_proof(
        &self,
        txid: &str,
        merkle_path: &str,
        block_headers: &[String],
    ) -> Result<bool, Status> {
        let start = std::time::Instant::now();
        self.proof_requests.inc();

        let tx_bytes = hex::decode(txid).map_err(|e| {
            warn!("Invalid txid: {}", e);
            let _ = self
                .send_alert(
                    "spv_verify_invalid_txid",
                    &format!("Invalid txid: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid txid: {}", e))
        })?;
        let _tx: Transaction = deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self
                .send_alert(
                    "spv_verify_invalid_transaction",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        let merkle_path_bytes = hex::decode(merkle_path).map_err(|e| {
            warn!("Invalid merkle path: {}", e);
            let _ = self
                .send_alert(
                    "spv_verify_invalid_merkle_path",
                    &format!("Invalid merkle path: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid merkle path: {}", e))
        })?;
        let _merkle_path: Vec<Sha256d> = deserialize(&merkle_path_bytes).map_err(|e| {
            warn!("Invalid merkle path deserialization: {}", e);
            let _ = self
                .send_alert(
                    "spv_verify_merkle_path_deserialization",
                    &format!("Invalid merkle path: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid merkle path deserialization: {}", e))
        })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(true) // Placeholder
    }
}

#[tonic::async_trait]
impl Validation for ValidationServiceImpl {
    async fn generate_spv_proof(
        &self,
        request: Request<GenerateSPVProofRequest>,
    ) -> Result<Response<GenerateSPVProofResponse>, Status> {
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
        self.authorize(&user_id, "GenerateSPVProof").await?;
        self.rate_limiter.until_ready().await;

        self.proof_requests.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let (merkle_path, block_headers) = self.generate_spv_proof(&req.txid).await?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(GenerateSPVProofResponse {
            success: true,
            merkle_path,
            block_headers,
            error: "".to_string(),
        }))
    }

    async fn verify_spv_proof(
        &self,
        request: Request<VerifySPVProofRequest>,
    ) -> Result<Response<VerifySPVProofResponse>, Status> {
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
        self.authorize(&user_id, "VerifySPVProof").await?;
        self.rate_limiter.until_ready().await;

        self.proof_requests.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let is_valid = self
            .verify_spv_proof(&req.txid, &req.merkle_path, &req.block_headers)
            .await?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(VerifySPVProofResponse {
            success: is_valid,
            error: if is_valid {
                "".to_string()
            } else {
                "Proof verification failed".to_string()
            },
        }))
    }

    async fn batch_generate_spv_proof(
        &self,
        request: Request<BatchGenerateSPVProofRequest>,
    ) -> Result<Response<BatchGenerateSPVProofResponse>, Status> {
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
        self.authorize(&user_id, "BatchGenerateSPVProof").await?;
        self.rate_limiter.until_ready().await;

        self.proof_requests.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut results = vec![];
        let mut proof_cache = self.proof_cache.lock().await;

        for txid in req.txids {
            let result = self.generate_spv_proof(&txid).await;
            match result {
                Ok((merkle_path_hex, block_headers)) => {
                    proof_cache.put(
                        txid.clone(),
                        (merkle_path_hex.clone(), block_headers.clone()),
                    );
                    results.push(GenerateSPVProofResponse {
                        success: true,
                        merkle_path: merkle_path_hex,
                        block_headers,
                        error: "".to_string(),
                    });
                }
                Err(e) => {
                    results.push(GenerateSPVProofResponse {
                        success: false,
                        merkle_path: "".to_string(),
                        block_headers: vec![],
                        error: e.to_string(),
                    });
                }
            }
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchGenerateSPVProofResponse { results }))
    }

    type StreamSPVProofsStream =
        Pin<Box<dyn Stream<Item = Result<StreamSPVProofsResponse, Status>> + Send>>;

    async fn stream_spv_proofs(
        &self,
        request: Request<Streaming<StreamSPVProofsRequest>>,
    ) -> Result<Response<Self::StreamSPVProofsStream>, Status> {
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
        self.authorize(&user_id, "StreamSPVProofs").await?;
        self.rate_limiter.until_ready().await;

        self.proof_requests.inc();
        let start = std::time::Instant::now();
        let mut stream = request.into_inner();
        let proof_requests = self.proof_requests.clone();
        let latency_ms = self.latency_ms.clone();
        let alert_client = self.alert_client.clone();
        let proof_cache = self.proof_cache.clone();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                proof_requests.inc();
                let start_inner = std::time::Instant::now();
                let result = self.generate_spv_proof(&req.txid).await;
                match result {
                    Ok((merkle_path_hex, block_headers)) => {
                        let mut proof_cache = proof_cache.lock().await;
                        proof_cache.put(req.txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
                        yield StreamSPVProofsResponse {
                            success: true,
                            merkle_path: merkle_path_hex,
                            block_headers,
                            error: "".to_string(),
                        };
                    }
                    Err(e) => {
                        warn!("Failed to generate SPV proof: {}", e);
                        let _ = alert_client.clone()
                            .send_alert(SendAlertRequest {
                                event_type: "spv_proof_generation_failed".to_string(),
                                message: format!("Failed to generate SPV proof: {}", e),
                                severity: 2,
                            })
                            .await;
                        yield StreamSPVProofsResponse {
                            success: false,
                            merkle_path: "".to_string(),
                            block_headers: vec![],
                            error: e.to_string(),
                        };
                    }
                }
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
            service_name: "validation_service".to_string(),
            requests_total: self.proof_requests.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: self.cache_hits.get() as u64,
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

    let addr = "[::1]:50057".parse().unwrap();
    let validation_service = ValidationServiceImpl::new().await;

    println!("Validation service listening on {}", addr);

    Server::builder()
        .add_service(ValidationServer::new(validation_service))
        .serve(addr)
        .await?;

    Ok(())
}
