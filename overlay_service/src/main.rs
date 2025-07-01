use tonic::{transport::{Server, Channel}, Request, Response, Status, Streaming};
use overlay::overlay_server::{Overlay, OverlayServer};
use overlay::{
    CreateOverlayRequest, CreateOverlayResponse, SubmitOverlayTxRequest, SubmitOverlayTxResponse,
    GetOverlayBlockRequest, GetOverlayBlockResponse, BatchSubmitOverlayTxRequest, BatchSubmitOverlayTxResponse,
    StreamOverlayTxRequest, StreamOverlayTxResponse, GetMetricsRequest, GetMetricsResponse
};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use network::network_client::NetworkClient;
use network::BroadcastTxRequest;
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
use toml;

tonic::include_proto!("overlay");
tonic::include_proto!("transaction");
tonic::include_proto!("block");
tonic::include_proto!("network");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct OverlayServiceImpl {
    transaction_client: TransactionClient<Channel>,
    block_client: BlockClient<Channel>,
    network_client: NetworkClient<Channel>,
    overlays: Arc<Mutex<HashMap<String, (Vec<Block>, Vec<Transaction>)>>>,
    db: Arc<Db>,
    registry: Arc<Registry>,
    tx_requests: Counter,
    latency_ms: Gauge,
    block_count: Counter,
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
        let overlays = Arc::new(Mutex::new(HashMap::new()));
        let db = Arc::new(sled::open(format!("overlay_db_{}", shard_id)).expect("Failed to open sled DB"));
        let registry = Arc::new(Registry::new());
        let tx_requests = Counter::new("overlay_tx_requests_total", "Total transaction requests").unwrap();
        let latency_ms = Gauge::new("overlay_latency_ms", "Average transaction processing latency").unwrap();
        let block_count = Counter::new("overlay_block_count", "Total overlay blocks created").unwrap();
        registry.register(Box::new(tx_requests.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(block_count.clone())).unwrap();
        OverlayServiceImpl { transaction_client, block_client, network_client, overlays, db, registry, tx_requests, latency_ms, block_count }
    }

    async fn assemble_overlay_block(&self, overlay_id: &str, transactions: Vec<Transaction>) -> Result<Block, String> {
        let txids: Vec<Sha256d> = transactions.iter().map(|tx| tx.txid()).collect();
        let mut merkle_root = if txids.is_empty() {
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
                bits: 0x1d00ffff,
                nonce: 0,
            },
            transactions,
        };

        self.db.insert(format!("block_{}:{}", overlay_id, self.block_count.get()), serialize(&block))
            .map_err(|e| format!("Failed to store block: {}", e))?;
        self.block_count.inc();
        Ok(block)
    }
}

#[tonic::async_trait]
impl Overlay for OverlayServiceImpl {
    async fn create_overlay(&self, request: Request<CreateOverlayRequest>) -> Result<Response<CreateOverlayResponse>, Status> {
        self.tx_requests.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        if overlays.contains_key(&req.overlay_id) {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(CreateOverlayResponse {
                success: false,
                error: "Overlay already exists".to_string(),
            }));
        }
        overlays.insert(req.overlay_id, (vec![], vec![]));
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(CreateOverlayResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn submit_overlay_transaction(&self, request: Request<SubmitOverlayTxRequest>) -> Result<Response<SubmitOverlayTxResponse>, Status> {
        self.tx_requests.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let (blocks, pending_txs) = overlays.get_mut(&req.overlay_id)
            .ok_or_else(|| Status::not_found("Overlay not found"))?;

        let mut transaction_client = self.transaction_client.clone();
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
        let validate_response = transaction_client.validate_transaction(validate_request)
            .await
            .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
            .into_inner();
        if !validate_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        let mut network_client = self.network_client.clone();
        let broadcast_request = BroadcastTxRequest { tx_hex: req.tx_hex.clone() };
        let broadcast_response = network_client.broadcast_transaction(broadcast_request)
            .await
            .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?
            .into_inner();
        if !broadcast_response.success {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: broadcast_response.error,
            }));
        }

        pending_txs.push(tx);
        if pending_txs.len() >= 100 {
            let block = self.assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                .await
                .map_err(|e| Status::internal(format!("Block assembly failed: {}", e)))?;
            blocks.push(block);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(SubmitOverlayTxResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_overlay_block(&self, request: Request<GetOverlayBlockRequest>) -> Result<Response<GetOverlayBlockResponse>, Status> {
        self.tx_requests.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let block_key = format!("block_{}:{}", req.overlay_id, req.block_height);
        let block_bytes = self.db.get(&block_key)
            .map_err(|e| Status::internal(format!("Failed to retrieve block: {}", e)))?
            .ok_or_else(|| Status::not_found("Block not found"))?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| Status::internal(format!("Invalid block: {}", e)))?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(GetOverlayBlockResponse {
            block_hex: hex::encode(serialize(&block)),
            error: "".to_string(),
        }))
    }

    async fn batch_submit_overlay_transaction(&self, request: Request<BatchSubmitOverlayTxRequest>) -> Result<Response<BatchSubmitOverlayTxResponse>, Status> {
        self.tx_requests.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let (blocks, pending_txs) = overlays.get_mut(&req.overlay_id)
            .ok_or_else(|| Status::not_found("Overlay not found"))?;

        let mut results = vec![];
        let mut transaction_client = self.transaction_client.clone();
        let mut network_client = self.network_client.clone();

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxRequest { tx_hex: tx_hex.clone() };
            let validate_response = transaction_client.validate_transaction(validate_request)
                .await
                .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
                .into_inner();
            if !validate_response.is_valid {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: validate_response.error,
                });
                continue;
            }

            let broadcast_request = BroadcastTxRequest { tx_hex: tx_hex.clone() };
            let broadcast_response = network_client.broadcast_transaction(broadcast_request)
                .await
                .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?
                .into_inner();
            if !broadcast_response.success {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: broadcast_response.error,
                });
                continue;
            }

            let tx_bytes = hex::decode(&tx_hex)
                .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
            let tx: Transaction = deserialize(&tx_bytes)
                .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;
            pending_txs.push(tx);

            results.push(SubmitOverlayTxResponse {
                success: true,
                error: "".to_string(),
            });
        }

        if pending_txs.len() >= 100 {
            let block = self.assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                .await
                .map_err(|e| Status::internal(format!("Block assembly failed: {}", e)))?;
            blocks.push(block);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchSubmitOverlayTxResponse { results }))
    }

    async fn stream_overlay_transactions(&self, request: Request<Streaming<StreamOverlayTxRequest>>) -> Result<Response<Self::StreamOverlayTxResponseStream>, Status> {
        self.tx_requests.inc();
        let start = Instant::now();
        let mut stream = request.into_inner();
        let overlays = Arc::clone(&self.overlays);
        let transaction_client = self.transaction_client.clone();
        let network_client = self.network_client.clone();
        let block_count = self.block_count.clone();
        let latency_ms = self.latency_ms.clone();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                let req = req?;
                let mut overlays = overlays.lock().await;
                let (blocks, pending_txs) = overlays.get_mut(&req.overlay_id)
                    .ok_or_else(|| Status::not_found("Overlay not found"))?;

                let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
                let validate_response = transaction_client.clone().validate_transaction(validate_request)
                    .await
                    .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
                    .into_inner();
                if !validate_response.is_valid {
                    yield StreamOverlayTxResponse {
                        success: false,
                        tx_hex: req.tx_hex,
                        error: validate_response.error,
                    };
                    continue;
                }

                let broadcast_request = BroadcastTxRequest { tx_hex: req.tx_hex.clone() };
                let broadcast_response = network_client.clone().broadcast_transaction(broadcast_request)
                    .await
                    .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?
                    .into_inner();
                if !broadcast_response.success {
                    yield StreamOverlayTxResponse {
                        success: false,
                        tx_hex: req.tx_hex,
                        error: broadcast_response.error,
                    };
                    continue;
                }

                let tx_bytes = hex::decode(&req.tx_hex)
                    .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
                let tx: Transaction = deserialize(&tx_bytes)
                    .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;
                pending_txs.push(tx);

                if pending_txs.len() >= 100 {
                    let block = self.assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                        .await
                        .map_err(|e| Status::internal(format!("Block assembly failed: {}", e)))?;
                    blocks.push(block);
                    block_count.inc();
                }

                yield StreamOverlayTxResponse {
                    success: true,
                    tx_hex: req.tx_hex,
                    error: "".to_string(),
                };
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(output)))
    }

    async fn get_metrics(&self, _request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        Ok(Response::new(GetMetricsResponse {
            service_name: "overlay_service".to_string(),
            requests_total: self.tx_requests.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50056".parse().unwrap();
    let overlay_service = OverlayServiceImpl::new().await;

    println!("Overlay service listening on {}", addr);

    Server::builder()
        .add_service(OverlayServer::new(overlay_service))
        .serve(addr)
        .await?;

    Ok(())
                }
