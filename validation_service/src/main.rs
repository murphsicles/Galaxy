use tonic::{transport::{Server, Channel}, Request, Response, Status, Streaming};
use validation::validation_server::{Validation, ValidationServer};
use validation::{
    GenerateSPVProofRequest, GenerateSPVProofResponse,
    VerifySPVProofRequest, VerifySPVProofResponse,
    BatchGenerateSPVProofRequest, BatchGenerateSPVProofResponse,
    StreamSPVProofsRequest, StreamSPVProofsResponse,
    GetMetricsRequest, GetMetricsResponse
};
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::block::Block;
use sv::util::{deserialize, serialize, hash::Sha256d};
use futures::StreamExt;
use lru::LruCache;
use tokio::sync::Mutex;
use std::sync::Arc;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter, Jitter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use toml;

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
    requests_total: Counter,
    latency_ms: Gauge,
    cache_hits: Counter,
    alert_count: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
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
        let proof_cache = Arc::new(Mutex::new(LruCache::new(1000)));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("validation_requests_total", "Total SPV proof requests").unwrap();
        let latency_ms = Gauge::new("validation_latency_ms", "Average proof generation latency").unwrap();
        let cache_hits = Counter::new("validation_cache_hits", "Cache hit count").unwrap();
        let alert_count = Counter::new("validation_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(cache_hits.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));
        let shard_manager = Arc::new(ShardManager::new());

        ValidationServiceImpl {
            block_client,
            storage_client,
            auth_client,
            alert_client,
            proof_cache,
            registry,
            requests_total,
            latency_ms,
            cache_hits,
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
            service: "validation_service".to_string(),
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

    async fn fetch_block(&self, txid: &str) -> Result<Block, String> {
        info!("Fetching block for txid: {}", txid);
        // Placeholder: Fetch block from block_service or storage
        Ok(Block {
            header: sv::block::BlockHeader {
                version: 1,
                prev_blockhash: Default::default(),
                merkle_root: Sha256d::from_hex("0000000000000000000000000000000000000000000000000000000000000000")
                    .unwrap(),
                time: 0,
                bits: 0x1d00ffff,
                nonce: 0,
            },
            transactions: vec![],
        })
    }

    async fn generate_merkle_path(&self, txid: &str, block: &Block) -> Result<(Vec<u8>, Sha256d), String> {
        let txids: Vec<Sha256d> = block.transactions.iter().map(|tx| tx.txid()).collect();
        let target_txid = Sha256d::from_hex(txid).map_err(|e| format!("Invalid txid: {}", e))?;

        let mut path = vec![];
        let mut current_level = txids;
        let mut index = current_level.iter().position(|id| *id == target_txid)
            .ok_or_else(|| {
                warn!("Transaction not found in block: {}", txid);
                "Transaction not found in block".to_string()
            })?;

        while current_level.len() > 1 {
            let mut next_level = vec![];
            for i in (0..current_level.len()).step_by(2) {
                let left = current_level[i];
                let right = if i + 1 < current_level.len() { current_level[i + 1] } else { left };
                if i == index || i + 1 == index {
                    path.extend_from_slice(if i == index { &right[..] } else { &left[..] });
                    index = i / 2;
                }
                let mut combined = [0u8; 64];
                combined[..32].copy_from_slice(&left[..]);
                combined[32..].copy_from_slice(&right[..]);
                let parent = Sha256d::double_sha256(&combined);
                next_level.push(parent);
            }
            current_level = next_level;
        }

        Ok((path, current_level[0]))
    }

    async fn validate_difficulty(&self, header: &sv::block::BlockHeader) -> Result<bool, String> {
        let target_bits = 0x1d00ffff; // Placeholder
        let target = Self::bits_to_target(target_bits);
        let hash = header.hash();
        Ok(hash <= target)
    }

    fn bits_to_target(bits: u32) -> [u8; 32] {
        let exponent = (bits >> 24) as u8;
        let mantissa = bits & 0x007fffff;
        let mut target = [0u8; 32];
        if exponent <= 3 {
            let value = mantissa >> (8 * (3 - exponent));
            target[31] = (value & 0xff) as u8;
            target[30] = ((value >> 8) & 0xff) as u8;
            target[29] = ((value >> 16) & 0xff) as u8;
        } else {
            let offset = 32 - exponent as usize;
            target[offset] = (mantissa & 0xff) as u8;
            target[offset + 1] = ((mantissa >> 8) & 0xff) as u8;
            target[offset + 2] = ((mantissa >> 16) & 0xff) as u8;
        }
        target
    }
}

#[tonic::async_trait]
impl Validation for ValidationServiceImpl {
    async fn generate_spv_proof(&self, request: Request<GenerateSPVProofRequest>) -> Result<Response<GenerateSPVProofResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GenerateSPVProof").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Generating SPV proof for txid: {}", request.get_ref().txid);
        let req = request.into_inner();
        let mut proof_cache = self.proof_cache.lock().await;
        if let Some((merkle_path, block_headers)) = proof_cache.get(&req.txid) {
            self.cache_hits.inc();
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            info!("Cache hit for txid: {}", req.txid);
            return Ok(Response::new(GenerateSPVProofResponse {
                success: true,
                merkle_path: merkle_path.clone(),
                block_headers: block_headers.clone(),
                error: "".to_string(),
            }));
        }

        let block = self.fetch_block(&req.txid)
            .await
            .map_err(|e| {
                warn!("Failed to fetch block: {}", e);
                let _ = self.send_alert("spv_proof_block_fetch_failed", &format!("Failed to fetch block: {}", e), 2).await;
                Status::internal(format!("Failed to fetch block: {}", e))
            })?;
        let (merkle_path, merkle_root) = self.generate_merkle_path(&req.txid, &block)
            .await
            .map_err(|e| {
                warn!("Failed to generate merkle path: {}", e);
                let _ = self.send_alert("spv_proof_merkle_path_failed", &format!("Failed to generate merkle path: {}", e), 2).await;
                Status::internal(format!("Failed to generate merkle path: {}", e))
            })?;

        if merkle_root != block.header.merkle_root {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Merkle root mismatch for txid: {}", req.txid);
            let _ = self.send_alert("spv_proof_merkle_root_mismatch", &format!("Merkle root mismatch for txid: {}", req.txid), 3).await;
            return Ok(Response::new(GenerateSPVProofResponse {
                success: false,
                merkle_path: "".to_string(),
                block_headers: vec![],
                error: "Merkle root mismatch".to_string(),
            }));
        }

        let block_headers = vec![hex::encode(serialize(&block.header))];
        let merkle_path_hex = hex::encode(&merkle_path);
        proof_cache.put(req.txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Generated SPV proof for txid: {}", req.txid);
        Ok(Response::new(GenerateSPVProofResponse {
            success: true,
            merkle_path: merkle_path_hex,
            block_headers,
            error: "".to_string(),
        }))
    }

    async fn verify_spv_proof(&self, request: Request<VerifySPVProofRequest>) -> Result<Response<VerifySPVProofResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "VerifySPVProof").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Verifying SPV proof for txid: {}", request.get_ref().txid);
        let req = request.into_inner();
        let merkle_path = hex::decode(&req.merkle_path)
            .map_err(|e| {
                warn!("Invalid merkle_path: {}", e);
                let _ = self.send_alert("spv_proof_invalid_format", &format!("Invalid merkle_path: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid merkle_path: {}", e))
            })?;
        let txid = Sha256d::from_hex(&req.txid)
            .map_err(|e| {
                warn!("Invalid txid: {}", e);
                let _ = self.send_alert("spv_proof_invalid_format", &format!("Invalid txid: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid txid: {}", e))
            })?;

        let mut prev_hash = None;
        for header_hex in req.block_headers {
            let header_bytes = hex::decode(&header_hex)
                .map_err(|e| {
                    warn!("Invalid block header: {}", e);
                    let _ = self.send_alert("spv_proof_invalid_format", &format!("Invalid block header: {}", e), 2).await;
                    Status::invalid_argument(format!("Invalid block header: {}", e))
                })?;
            let header: sv::block::BlockHeader = deserialize(&header_bytes)
                .map_err(|e| {
                    warn!("Invalid block header: {}", e);
                    let _ = self.send_alert("spv_proof_invalid_format", &format!("Invalid block header: {}", e), 2).await;
                    Status::invalid_argument(format!("Invalid block header: {}", e))
                })?;

            if !self.validate_difficulty(&header).await
                .map_err(|e| {
                    warn!("Difficulty validation failed: {}", e);
                    let _ = self.send_alert("spv_proof_difficulty_failed", &format!("Difficulty validation failed: {}", e), 2).await;
                    Status::internal(format!("Difficulty validation failed: {}", e))
                })?
            {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                warn!("Invalid difficulty for txid: {}", req.txid);
                let _ = self.send_alert("spv_proof_difficulty_failed", &format!("Invalid difficulty for txid: {}", req.txid), 2).await;
                return Ok(Response::new(VerifySPVProofResponse {
                    is_valid: false,
                    error: "Invalid difficulty".to_string(),
                }));
            }

            if let Some(prev) = prev_hash {
                if header.prev_blockhash != prev {
                    self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                    warn!("Invalid header chain for txid: {}", req.txid);
                    let _ = self.send_alert("spv_proof_header_chain_failed", &format!("Invalid header chain for txid: {}", req.txid), 3).await;
                    return Ok(Response::new(VerifySPVProofResponse {
                        is_valid: false,
                        error: "Invalid header chain".to_string(),
                    }));
                }
            }
            prev_hash = Some(header.hash());
        }

        let mut current_hash = txid;
        for i in (0..merkle_path.len()).step_by(32) {
            let sibling = &merkle_path[i..i + 32];
            let mut combined = [0u8; 64];
            if i % 64 == 0 {
                combined[..32].copy_from_slice(&current_hash[..]);
                combined[32..].copy_from_slice(sibling);
            } else {
                combined[..32].copy_from_slice(sibling);
                combined[32..].copy_from_slice(&current_hash[..]);
            }
            current_hash = Sha256d::double_sha256(&combined);
        }

        let is_valid = current_hash == prev_hash.unwrap_or_default();
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        if !is_valid {
            let _ = self.send_alert("spv_proof_verification_failed", &format!("Merkle path verification failed for txid: {}", req.txid), 3).await;
        }
        info!("SPV proof verification {} for txid: {}", if is_valid { "succeeded" } else { "failed" }, req.txid);
        Ok(Response::new(VerifySPVProofResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Merkle path verification failed".to_string() },
        }))
    }

    async fn batch_generate_spv_proof(&self, request: Request<BatchGenerateSPVProofRequest>) -> Result<Response<BatchGenerateSPVProofResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BatchGenerateSPVProof").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Batch generating SPV proofs for {} txids", request.get_ref().txids.len());
        let req = request.into_inner();
        let mut results = vec![];
        let mut proof_cache = self.proof_cache.lock().await;

        for txid in req.txids {
            if let Some((merkle_path, block_headers)) = proof_cache.get(&txid) {
                self.cache_hits.inc();
                results.push(GenerateSPVProofResponse {
                    success: true,
                    merkle_path: merkle_path.clone(),
                    block_headers: block_headers.clone(),
                    error: "".to_string(),
                });
                info!("Cache hit for txid: {}", txid);
                continue;
            }

            let block = self.fetch_block(&txid)
                .await
                .map_err(|e| {
                    warn!("Failed to fetch block: {}", e);
                    let _ = self.send_alert("spv_proof_block_fetch_failed", &format!("Failed to fetch block: {}", e), 2).await;
                    Status::internal(format!("Failed to fetch block: {}", e))
                })?;
            let (merkle_path, merkle_root) = self.generate_merkle_path(&txid, &block)
                .await
                .map_err(|e| {
                    warn!("Failed to generate merkle path: {}", e);
                    let _ = self.send_alert("spv_proof_merkle_path_failed", &format!("Failed to generate merkle path: {}", e), 2).await;
                    Status::internal(format!("Failed to generate merkle path: {}", e))
                })?;

            if merkle_root != block.header.merkle_root {
                results.push(GenerateSPVProofResponse {
                    success: false,
                    merkle_path: "".to_string(),
                    block_headers: vec![],
                    error: "Merkle root mismatch".to_string(),
                });
                let _ = self.send_alert("spv_proof_merkle_root_mismatch", &format!("Merkle root mismatch for txid: {}", txid), 3).await;
                warn!("Merkle root mismatch for txid: {}", txid);
                continue;
            }

            let block_headers = vec![hex::encode(serialize(&block.header))];
            let merkle_path_hex = hex::encode(&merkle_path);
            proof_cache.put(txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
            results.push(GenerateSPVProofResponse {
                success: true,
                merkle_path: merkle_path_hex,
                block_headers,
                error: "".to_string(),
            });
            info!("Generated SPV proof for txid: {}", txid);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchGenerateSPVProofResponse { results }))
    }

    async fn stream_spv_proofs(&self, request: Request<Streaming<StreamSPVProofsRequest>>) -> Result<Response<Self::StreamSPVProofsStream>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "StreamSPVProofs").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        let mut stream = request.into_inner();
        let proof_cache = Arc::clone(&self.proof_cache);
        let requests_total = self.requests_total.clone();
        let cache_hits = self.cache_hits.clone();
        let latency_ms = self.latency_ms.clone();
        let alert_client = self.alert_client.clone();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                requests_total.inc();
                let req = req?;
                info!("Streaming SPV proof for txid: {}", req.txid);
                let mut proof_cache = proof_cache.lock().await;
                if let Some((merkle_path, block_headers)) = proof_cache.get(&req.txid) {
                    cache_hits.inc();
                    yield StreamSPVProofsResponse {
                        success: true,
                        txid: req.txid,
                        merkle_path: merkle_path.clone(),
                        block_headers: block_headers.clone(),
                        error: "".to_string(),
                    };
                    info!("Cache hit for streamed txid: {}", req.txid);
                    continue;
                }

                let block = self.fetch_block(&req.txid)
                    .await
                    .map_err(|e| {
                        let _ = alert_client.clone()
                            .send_alert(SendAlertRequest {
                                event_type: "spv_proof_block_fetch_failed".to_string(),
                                message: format!("Failed to fetch block: {}", e),
                                severity: 2,
                            })
                            .await;
                        Status::internal(format!("Failed to fetch block: {}", e))
                    })?;
                let (merkle_path, merkle_root) = self.generate_merkle_path(&req.txid, &block)
                    .await
                    .map_err(|e| {
                        let _ = alert_client.clone()
                            .send_alert(SendAlertRequest {
                                event_type: "spv_proof_merkle_path_failed".to_string(),
                                message: format!("Failed to generate merkle path: {}", e),
                                severity: 2,
                            })
                            .await;
                        Status::internal(format!("Failed to generate merkle path: {}", e))
                    })?;

                if merkle_root != block.header.merkle_root {
                    let _ = alert_client.clone()
                        .send_alert(SendAlertRequest {
                            event_type: "spv_proof_merkle_root_mismatch".to_string(),
                            message: format!("Merkle root mismatch for txid: {}", req.txid),
                            severity: 3,
                        })
                        .await;
                    yield StreamSPVProofsResponse {
                        success: false,
                        txid: req.txid,
                        merkle_path: "".to_string(),
                        block_headers: vec![],
                        error: "Merkle root mismatch".to_string(),
                    };
                    warn!("Merkle root mismatch for streamed txid: {}", req.txid);
                    continue;
                }

                let block_headers = vec![hex::encode(serialize(&block.header))];
                let merkle_path_hex = hex::encode(&merkle_path);
                proof_cache.put(req.txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
                yield StreamSPVProofsResponse {
                    success: true,
                    txid: req.txid,
                    merkle_path: merkle_path_hex,
                    block_headers,
                    error: "".to_string(),
                };
                info!("Generated streamed SPV proof for txid: {}", req.txid);
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(output)))
    }

    async fn get_metrics(&self, request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetMetrics").await?;

        Ok(Response::new(GetMetricsResponse {
            service_name: "validation_service".to_string(),
            requests_total: self.requests_total.get() as u64,
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
