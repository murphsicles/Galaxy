use tonic::{transport::{Server, Channel}, Request, Response, Status, Streaming};
use overlay::overlay_server::{Overlay, OverlayServer};
use overlay::{
    CreateOverlayRequest, CreateOverlayResponse, SubmitOverlayTxRequest, SubmitOverlayTxResponse,
    GetOverlayBlockRequest, GetOverlayBlockResponse, BatchSubmitOverlayTxRequest, BatchSubmitOverlayTxResponse,
    StreamOverlayTxRequest, StreamOverlayTxResponse, IndexOverlayTransactionRequest, IndexOverlayTransactionResponse,
    QueryOverlayBlockRequest, QueryOverlayBlockResponse, ManageOverlayConsensusRequest, ManageOverlayConsensusResponse,
    GetMetricsRequest, GetMetricsResponse
};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use network::network_client::NetworkClient;
use network::BroadcastTxRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use index::index_client::IndexClient;
use index::IndexTransactionRequest as IndexIndexTransactionRequest;
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::transaction::Transaction;
use sv::block::Block;
use sv::util::{deserialize, serialize, hash::Sha256d};
use futures::StreamExt;
use sled::Db;
use hex;
use std::collections::HashMap;
use tokio::sync::Mutex;
use std::sync::Arc;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter, Jitter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use toml;

tonic::include_proto!("overlay");
tonic::include_proto!("transaction");
tonic::include_proto!("block");
tonic::include_proto!("network");
tonic::include_proto!("auth");
tonic::include_proto!("index");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct OverlayServiceImpl {
    transaction_client: TransactionClient<Channel>,
    block_client: BlockClient<Channel>,
    network_client: NetworkClient<Channel>,
    auth_client: AuthClient<Channel>,
    index_client: IndexClient<Channel>,
    alert_client: AlertClient<Channel>,
    overlays: Arc<Mutex<HashMap<String, (Vec<Block>, Vec<Transaction>)>>>,
    consensus_rules: Arc<Mutex<HashMap<String, HashMap<String, bool>>>>, // Overlay ID -> Rule Name -> Enabled
    db: Arc<Db>,
    registry: Arc<Registry>,
    tx_requests: Counter,
    latency_ms: Gauge,
    block_count: Counter,
    alert_count: Counter,
    index_throughput: Gauge,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl OverlayServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let transaction_client = TransactionClient::connect("http://[::1]:50052")
            .await
            .expect("Failed to connect to transaction_service");
        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let network_client = NetworkClient::connect("http://[::1]:50051")
            .await
            .expect("Failed to connect to network_service");
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let index_client = IndexClient::connect("http://[::1]:50062")
            .await
            .expect("Failed to connect to index_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let overlays = Arc::new(Mutex::new(HashMap::new()));
        let consensus_rules = Arc::new(Mutex::new(HashMap::new()));
        let db = Arc::new(sled::open(format!("overlay_db_{}", shard_id)).expect("Failed to open sled DB"));
        let registry = Arc::new(Registry::new());
        let tx_requests = Counter::new("overlay_tx_requests_total", "Total transaction requests").unwrap();
        let latency_ms = Gauge::new("overlay_latency_ms", "Average transaction processing latency").unwrap();
        let block_count = Counter::new("overlay_block_count", "Total overlay blocks created").unwrap();
        let alert_count = Counter::new("overlay_alert_count", "Total alerts sent").unwrap();
        let index_throughput = Gauge::new("overlay_index_throughput", "Indexed overlay transactions per second").unwrap();
        registry.register(Box::new(tx_requests.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(block_count.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(index_throughput.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));
        let shard_manager = Arc::new(ShardManager::new());

        OverlayServiceImpl {
            transaction_client,
            block_client,
            network_client,
            auth_client,
            index_client,
            alert_client,
            overlays,
            consensus_rules,
            db,
            registry,
            tx_requests,
            latency_ms,
            block_count,
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
            service: "overlay_service".to_string(),
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

    async fn assemble_overlay_block(&self, overlay_id: &str, transactions: Vec<Transaction>) -> Result<Block, String> {
        let txids: Vec<Sha256d> = transactions.iter().map(|tx| tx.txid()).collect();
        let merkle_root = if txids.is_empty() {
            Sha256d::from_hex("0000000000000000000000000000000000000000000000000000000000000000").unwrap()
        } else {
            let mut current_level = txids;
            while current_level.len() > 1 {
                let mut next_level = vec![];
                for i in (0..current_level.len()).step_by(2) {
                    let left = current_level[i];
                    let right = if i + 1 < current_level.len() { current_level[i + 1] } else { left };
                    let mut combined = [0u8; 64];
                    combined[..32].copy_from_slice(&left[..]);
                    combined[32..].copy_from_slice(&right[..]);
                    let parent = Sha256d::double_sha256(&combined);
                    next_level.push(parent);
                }
                current_level = next_level;
            }
            current_level[0]
        };

        let block = Block {
            header: sv::block::BlockHeader {
                version: 1,
                prev_blockhash: Default::default(),
                merkle_root,
                time: 0,
                bits: 0x1d00ffff, // Placeholder
                nonce: 0,
            },
            transactions,
        };

        let block_key = format!("block_{}:{}", overlay_id, self.block_count.get());
        self.db
            .insert(&block_key, serialize(&block))
            .map_err(|e| format!("Failed to store block: {}", e))?;
        self.block_count.inc();
        info!("Assembled overlay block for {}: {}", overlay_id, block_key);
        Ok(block)
    }

    async fn validate_overlay_consensus(&self, overlay_id: &str, tx: &Transaction) -> Result<bool, String> {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let max_op_return_size = config["overlay_consensus"]["max_op_return_size"]
            .as_integer()
            .unwrap_or(4294967296) as usize; // 4.3GB default

        let consensus_rules = self.consensus_rules.lock().await;
        let rules = consensus_rules.get(overlay_id).ok_or_else(|| {
            warn!("No consensus rules for overlay: {}", overlay_id);
            "No consensus rules defined".to_string()
        })?;

        if let Some(&enabled) = rules.get("restrict_op_return") {
            if enabled {
                for output in &tx.outputs {
                    if output.script_pubkey.is_op_return() && output.script_pubkey.len() > max_op_return_size {
                        warn!("OP_RETURN exceeds 4.3GB limit for overlay: {}", overlay_id);
                        let _ = self.send_alert(
                            "overlay_op_return_exceeded",
                            &format!("OP_RETURN exceeds 4.3GB limit for overlay: {}", overlay_id),
                            3,
                        ).await;
                        return Err("OP_RETURN exceeds 4.3GB limit".to_string());
                    }
                }
            }
        }

        info!("Validated overlay consensus for txid: {} in overlay: {}", tx.txid(), overlay_id);
        Ok(true)
    }
}

#[tonic::async_trait]
impl Overlay for OverlayServiceImpl {
    async fn create_overlay(&self, request: Request<CreateOverlayRequest>) -> Result<Response<CreateOverlayResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "CreateOverlay").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Creating overlay: {}", request.get_ref().overlay_id);
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let mut consensus_rules = self.consensus_rules.lock().await;
        if overlays.contains_key(&req.overlay_id) {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Overlay already exists: {}", req.overlay_id);
            let _ = self.send_alert("overlay_creation_failed", &format!("Overlay {} already exists", req.overlay_id), 2).await;
            return Ok(Response::new(CreateOverlayResponse {
                success: false,
                error: "Overlay already exists".to_string(),
            }));
        }
        overlays.insert(req.overlay_id.clone(), (vec![], vec![]));
        consensus_rules.insert(req.overlay_id.clone(), HashMap::new());
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Created overlay: {}", req.overlay_id);
        Ok(Response::new(CreateOverlayResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn submit_overlay_transaction(&self, request: Request<SubmitOverlayTxRequest>) -> Result<Response<SubmitOverlayTxResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "SubmitOverlayTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Submitting overlay transaction: {}", request.get_ref().tx_hex);
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let (blocks, pending_txs) = overlays
            .get_mut(&req.overlay_id)
            .ok_or_else(|| {
                warn!("Overlay not found: {}", req.overlay_id);
                Status::not_found("Overlay not found")
            })?;

        let mut transaction_client = self.transaction_client.clone();
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
        let validate_response = transaction_client
            .validate_transaction(validate_request)
            .await
            .map_err(|e| {
                warn!("Transaction validation failed: {}", e);
                Status::internal(format!("Transaction validation failed: {}", e))
            })?
            .into_inner();
        if !validate_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Transaction validation failed: {}", validate_response.error);
            let _ = self.send_alert("overlay_tx_validation_failed", &format!("Transaction validation failed: {}", validate_response.error), 2).await;
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| {
                warn!("Invalid tx_hex: {}", e);
                Status::invalid_argument(format!("Invalid tx_hex: {}", e))
            })?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| {
                warn!("Invalid transaction: {}", e);
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

        if !self.validate_overlay_consensus(&req.overlay_id, &tx).await
            .map_err(|e| {
                warn!("Overlay consensus validation failed: {}", e);
                Status::invalid_argument(format!("Overlay consensus validation failed: {}", e))
            })?
        {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Overlay consensus validation failed for txid: {}", tx.txid());
            let _ = self.send_alert("overlay_consensus_failed", &format!("Overlay consensus validation failed for txid: {}", tx.txid()), 3).await;
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: "Overlay consensus validation failed".to_string(),
            }));
        }

        let mut index_client = self.index_client.clone();
        let index_request = IndexIndexTransactionRequest { tx_hex: req.tx_hex.clone() };
        let index_response = index_client
            .index_transaction(index_request)
            .await
            .map_err(|e| {
                warn!("Transaction indexing failed: {}", e);
                Status::internal(format!("Transaction indexing failed: {}", e))
            })?
            .into_inner();
        if !index_response.success {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Transaction indexing failed: {}", index_response.error);
            let _ = self.send_alert("overlay_tx_indexing_failed", &format!("Transaction indexing failed: {}", index_response.error), 2).await;
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: index_response.error,
            }));
        }
        self.index_throughput.inc_by(1.0);

        let mut network_client = self.network_client.clone();
        let broadcast_request = BroadcastTxRequest { tx_hex: req.tx_hex.clone() };
        let broadcast_response = network_client
            .broadcast_transaction(broadcast_request)
            .await
            .map_err(|e| {
                warn!("Broadcast failed: {}", e);
                Status::internal(format!("Broadcast failed: {}", e))
            })?
            .into_inner();
        if !broadcast_response.success {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Broadcast failed: {}", broadcast_response.error);
            let _ = self.send_alert("overlay_tx_broadcast_failed", &format!("Broadcast failed: {}", broadcast_response.error), 2).await;
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: broadcast_response.error,
            }));
        }

        pending_txs.push(tx);
        if pending_txs.len() >= 100 {
            let block = self
                .assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                .await
                .map_err(|e| {
                    warn!("Block assembly failed: {}", e);
                    Status::internal(format!("Block assembly failed: {}", e))
                })?;
            blocks.push(block);
            info!("Assembled overlay block for {}", req.overlay_id);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully submitted overlay transaction: {}", req.tx_hex);
        Ok(Response::new(SubmitOverlayTxResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_overlay_block(&self, request: Request<GetOverlayBlockRequest>) -> Result<Response<GetOverlayBlockResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetOverlayBlock").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Getting overlay block: {}:{}", request.get_ref().overlay_id, request.get_ref().block_height);
        let req = request.into_inner();
        let block_key = format!("block_{}:{}", req.overlay_id, req.block_height);
        let block_bytes = self
            .db
            .get(&block_key)
            .map_err(|e| {
                warn!("Failed to retrieve block {}: {}", block_key, e);
                Status::internal(format!("Failed to retrieve block: {}", e))
            })?
            .ok_or_else(|| {
                warn!("Block not found: {}", block_key);
                Status::not_found("Block not found")
            })?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| {
                warn!("Invalid block: {}", e);
                Status::internal(format!("Invalid block: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Retrieved overlay block: {}:{}", req.overlay_id, req.block_height);
        Ok(Response::new(GetOverlayBlockResponse {
            block_hex: hex::encode(serialize(&block)),
            error: "".to_string(),
        }))
    }

    async fn batch_submit_overlay_transaction(&self, request: Request<BatchSubmitOverlayTxRequest>) -> Result<Response<BatchSubmitOverlayTxResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BatchSubmitOverlayTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Batch submitting {} overlay transactions", request.get_ref().tx_hexes.len());
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let (blocks, pending_txs) = overlays
            .get_mut(&req.overlay_id)
            .ok_or_else(|| {
                warn!("Overlay not found: {}", req.overlay_id);
                Status::not_found("Overlay not found")
            })?;

        let mut results = vec![];
        let mut transaction_client = self.transaction_client.clone();
        let mut network_client = self.network_client.clone();
        let mut index_client = self.index_client.clone();

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxRequest { tx_hex: tx_hex.clone() };
            let validate_response = transaction_client
                .validate_transaction(validate_request)
                .await
                .map_err(|e| {
                    warn!("Transaction validation failed: {}", e);
                    Status::internal(format!("Transaction validation failed: {}", e))
                })?
                .into_inner();
            if !validate_response.is_valid {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: validate_response.error.clone(),
                });
                let _ = self.send_alert("overlay_tx_validation_failed", &format!("Batch transaction validation failed: {}", validate_response.error), 2).await;
                warn!("Batch transaction validation failed: {}", validate_response.error);
                continue;
            }

            let tx_bytes = hex::decode(&tx_hex)
                .map_err(|e| {
                    warn!("Invalid tx_hex: {}", e);
                    Status::invalid_argument(format!("Invalid tx_hex: {}", e))
                })?;
            let tx: Transaction = deserialize(&tx_bytes)
                .map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    Status::invalid_argument(format!("Invalid transaction: {}", e))
                })?;

            if !self.validate_overlay_consensus(&req.overlay_id, &tx).await
                .map_err(|e| {
                    warn!("Overlay consensus validation failed: {}", e);
                    Status::invalid_argument(format!("Overlay consensus validation failed: {}", e))
                })?
            {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: "Overlay consensus validation failed".to_string(),
                });
                let _ = self.send_alert("overlay_consensus_failed", &format!("Batch overlay consensus validation failed for txid: {}", tx.txid()), 3).await;
                warn!("Batch overlay consensus validation failed for txid: {}", tx.txid());
                continue;
            }

            let index_request = IndexIndexTransactionRequest { tx_hex: tx_hex.clone() };
            let index_response = index_client
                .index_transaction(index_request)
                .await
                .map_err(|e| {
                    warn!("Transaction indexing failed: {}", e);
                    Status::internal(format!("Transaction indexing failed: {}", e))
                })?
                .into_inner();
            if !index_response.success {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: index_response.error.clone(),
                });
                let _ = self.send_alert("overlay_tx_indexing_failed", &format!("Batch transaction indexing failed: {}", index_response.error), 2).await;
                warn!("Batch transaction indexing failed: {}", index_response.error);
                continue;
            }
            self.index_throughput.inc_by(1.0);

            let broadcast_request = BroadcastTxRequest { tx_hex: tx_hex.clone() };
            let broadcast_response = network_client
                .broadcast_transaction(broadcast_request)
                .await
                .map_err(|e| {
                    warn!("Broadcast failed: {}", e);
                    Status::internal(format!("Broadcast failed: {}", e))
                })?
                .into_inner();
            if !broadcast_response.success {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: broadcast_response.error.clone(),
                });
                let _ = self.send_alert("overlay_tx_broadcast_failed", &format!("Batch broadcast failed: {}", broadcast_response.error), 2).await;
                warn!("Batch broadcast failed: {}", broadcast_response.error);
                continue;
            }

            pending_txs.push(tx);
            results.push(SubmitOverlayTxResponse {
                success: true,
                error: "".to_string(),
            });
        }

        if pending_txs.len() >= 100 {
            let block = self
                .assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                .await
                .map_err(|e| {
                    warn!("Block assembly failed: {}", e);
                    Status::internal(format!("Block assembly failed: {}", e))
                })?;
            blocks.push(block);
            info!("Assembled batch overlay block for {}", req.overlay_id);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Completed batch submission for {} transactions", results.len());
        Ok(Response::new(BatchSubmitOverlayTxResponse { results }))
    }

    async fn stream_overlay_transactions(&self, request: Request<Streaming<StreamOverlayTxRequest>>) -> Result<Response<Self::StreamOverlayTxResponseStream>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "StreamOverlayTransactions").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Streaming overlay transactions");
        let mut stream = request.into_inner();
        let overlays = Arc::clone(&self.overlays);
        let transaction_client = self.transaction_client.clone();
        let network_client = self.network_client.clone();
        let index_client = self.index_client.clone();
        let alert_client = self.alert_client.clone();
        let block_count = self.block_count.clone();
        let latency_ms = self.latency_ms.clone();
        let index_throughput = self.index_throughput.clone();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                let req = req?;
                info!("Streaming overlay transaction: {}", req.tx_hex);
                let mut overlays = overlays.lock().await;
                let (blocks, pending_txs) = overlays
                    .get_mut(&req.overlay_id)
                    .ok_or_else(|| {
                        warn!("Overlay not found: {}", req.overlay_id);
                        Status::not_found("Overlay not found")
                    })?;

                let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
                let validate_response = transaction_client.clone()
                    .validate_transaction(validate_request)
                    .await
                    .map_err(|e| {
                        warn!("Transaction validation failed: {}", e);
                        Status::internal(format!("Transaction validation failed: {}", e))
                    })?
                    .into_inner();
                if !validate_response.is_valid {
                    let _ = alert_client.clone()
                        .send_alert(SendAlertRequest {
                            event_type: "overlay_tx_validation_failed".to_string(),
                            message: format!("Streamed transaction validation failed: {}", validate_response.error),
                            severity: 2,
                        })
                        .await;
                    yield StreamOverlayTxResponse {
                        success: false,
                        tx_hex: req.tx_hex,
                        error: validate_response.error,
                    };
                    warn!("Streamed transaction validation failed: {}", validate_response.error);
                    continue;
                }

                let tx_bytes = hex::decode(&req.tx_hex)
                    .map_err(|e| {
                        warn!("Invalid tx_hex: {}", e);
                        Status::invalid_argument(format!("Invalid tx_hex: {}", e))
                    })?;
                let tx: Transaction = deserialize(&tx_bytes)
                    .map_err(|e| {
                        warn!("Invalid transaction: {}", e);
                        Status::invalid_argument(format!("Invalid transaction: {}", e))
                    })?;

                if !self.validate_overlay_consensus(&req.overlay_id, &tx).await
                    .map_err(|e| {
                        warn!("Overlay consensus validation failed: {}", e);
                        Status::invalid_argument(format!("Overlay consensus validation failed: {}", e))
                    })?
                {
                    let _ = alert_client.clone()
                        .send_alert(SendAlertRequest {
                            event_type: "overlay_consensus_failed".to_string(),
                            message: format!("Streamed overlay consensus validation failed for txid: {}", tx.txid()),
                            severity: 3,
                        })
                        .await;
                    yield StreamOverlayTxResponse {
                        success: false,
                        tx_hex: req.tx_hex,
                        error: "Overlay consensus validation failed".to_string(),
                    };
                    warn!("Streamed overlay consensus validation failed for txid: {}", tx.txid());
                    continue;
                }

                let index_request = IndexIndexTransactionRequest { tx_hex: req.tx_hex.clone() };
                let index_response = index_client.clone()
                    .index_transaction(index_request)
                    .await
                    .map_err(|e| {
                        warn!("Transaction indexing failed: {}", e);
                        Status::internal(format!("Transaction indexing failed: {}", e))
                    })?
                    .into_inner();
                if !index_response.success {
                    let _ = alert_client.clone()
                        .send_alert(SendAlertRequest {
                            event_type: "overlay_tx_indexing_failed".to_string(),
                            message: format!("Streamed transaction indexing failed: {}", index_response.error),
                            severity: 2,
                        })
                        .await;
                    yield StreamOverlayTxResponse {
                        success: false,
                        tx_hex: req.tx_hex,
                        error: index_response.error,
                    };
                    warn!("Streamed transaction indexing failed: {}", index_response.error);
                    continue;
                }
                index_throughput.inc_by(1.0);

                let broadcast_request = BroadcastTxRequest { tx_hex: req.tx_hex.clone() };
                let broadcast_response = network_client.clone()
                    .broadcast_transaction(broadcast_request)
                    .await
                    .map_err(|e| {
                        warn!("Broadcast failed: {}", e);
                        Status::internal(format!("Broadcast failed: {}", e))
                    })?
                    .into_inner();
                if !broadcast_response.success {
                    let _ = alert_client.clone()
                        .send_alert(SendAlertRequest {
                            event_type: "overlay_tx_broadcast_failed".to_string(),
                            message: format!("Streamed broadcast failed: {}", broadcast_response.error),
                            severity: 2,
                        })
                        .await;
                    yield StreamOverlayTxResponse {
                        success: false,
                        tx_hex: req.tx_hex,
                        error: broadcast_response.error,
                    };
                    warn!("Streamed broadcast failed: {}", broadcast_response.error);
                    continue;
                }

                pending_txs.push(tx);
                if pending_txs.len() >= 100 {
                    let block = self
                        .assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                        .await
                        .map_err(|e| {
                            warn!("Block assembly failed: {}", e);
                            Status::internal(format!("Block assembly failed: {}", e))
                        })?;
                    blocks.push(block);
                    block_count.inc();
                    info!("Assembled streamed overlay block for {}", req.overlay_id);
                }

                yield StreamOverlayTxResponse {
                    success: true,
                    tx_hex: req.tx_hex,
                    error: "".to_string(),
                };
                info!("Streamed overlay transaction: {}", req.tx_hex);
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(output)))
    }

    async fn index_overlay_transaction(&self, request: Request<IndexOverlayTransactionRequest>) -> Result<Response<IndexOverlayTransactionResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "IndexOverlayTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Indexing overlay transaction: {} for overlay: {}", request.get_ref().tx_hex, request.get_ref().overlay_id);
        let req = request.into_inner();
        let mut index_client = self.index_client.clone();
        let index_request = IndexIndexTransactionRequest { tx_hex: req.tx_hex.clone() };
        let index_response = index_client
            .index_transaction(index_request)
            .await
            .map_err(|e| {
                warn!("Transaction indexing failed: {}", e);
                Status::internal(format!("Transaction indexing failed: {}", e))
            })?
            .into_inner();

        if !index_response.success {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Overlay transaction indexing failed: {}", index_response.error);
            let _ = self.send_alert("overlay_tx_indexing_failed", &format!("Overlay transaction indexing failed: {}", index_response.error), 2).await;
            return Ok(Response::new(IndexOverlayTransactionResponse {
                success: false,
                error: index_response.error,
            }));
        }
        self.index_throughput.inc_by(1.0);

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully indexed overlay transaction: {}", req.tx_hex);
        Ok(Response::new(IndexOverlayTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn query_overlay_block(&self, request: Request<QueryOverlayBlockRequest>) -> Result<Response<QueryOverlayBlockResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "QueryOverlayBlock").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Querying overlay block: {}:{}", request.get_ref().overlay_id, request.get_ref().block_height);
        let req = request.into_inner();
        let block_key = format!("block_{}:{}", req.overlay_id, req.block_height);
        let result = self.db
            .get(&block_key)
            .map_err(|e| {
                warn!("Failed to query block {}: {}", block_key, e);
                Status::internal(format!("Failed to query block: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        if let Some(block_bytes) = result {
            info!("Found overlay block: {}:{}", req.overlay_id, req.block_height);
            Ok(Response::new(QueryOverlayBlockResponse {
                block_hex: hex::encode(block_bytes),
                error: "".to_string(),
            }))
        } else {
            warn!("Overlay block not found: {}:{}", req.overlay_id, req.block_height);
            let _ = self.send_alert("overlay_block_query_failed", &format!("Overlay block not found: {}:{}", req.overlay_id, req.block_height), 2).await;
            Ok(Response::new(QueryOverlayBlockResponse {
                block_hex: "".to_string(),
                error: "Block not found".to_string(),
            }))
        }
    }

    async fn manage_overlay_consensus(&self, request: Request<ManageOverlayConsensusRequest>) -> Result<Response<ManageOverlayConsensusResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "ManageOverlayConsensus").await?;
        self.rate_limiter.until_ready().await;

        self.tx_requests.inc();
        let start = Instant::now();
        info!("Managing consensus rule for overlay: {} - rule: {}, enable: {}", request.get_ref().overlay_id, request.get_ref().rule_name, request.get_ref().enable);
        let req = request.into_inner();
        let mut consensus_rules = self.consensus_rules.lock().await;
        let rules = consensus_rules
            .entry(req.overlay_id.clone())
            .or_insert_with(HashMap::new);

        if req.rule_name != "restrict_op_return" {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            warn!("Unsupported consensus rule: {}", req.rule_name);
            let _ = self.send_alert("overlay_consensus_rule_unsupported", &format!("Unsupported consensus rule: {}", req.rule_name), 2).await;
            return Ok(Response::new(ManageOverlayConsensusResponse {
                success: false,
                error: "Unsupported consensus rule".to_string(),
            }));
        }

        rules.insert(req.rule_name.clone(), req.enable);
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Updated consensus rule for overlay: {} - {}: {}", req.overlay_id, req.rule_name, req.enable);
        Ok(Response::new(ManageOverlayConsensusResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_metrics(&self, request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetMetrics").await?;

        Ok(Response::new(GetMetricsResponse {
            service_name: "overlay_service".to_string(),
            requests_total: self.tx_requests.get() as u64,
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

    let addr = "[::1]:50056".parse().unwrap();
    let overlay_service = OverlayServiceImpl::new().await;

    println!("Overlay service listening on {}", addr);

    Server::builder()
        .add_service(OverlayServer::new(overlay_service))
        .serve(addr)
        .await?;

    Ok(())
}
