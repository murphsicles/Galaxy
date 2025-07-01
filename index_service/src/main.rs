use tonic::{transport::Server, Request, Response, Status};
use index::index_server::{Index, IndexServer};
use index::{
    IndexTransactionRequest, IndexTransactionResponse, QueryTransactionRequest, QueryTransactionResponse,
    BatchIndexTransactionsRequest, BatchIndexTransactionsResponse, IndexBlockRequest, IndexBlockResponse,
    QueryBlockRequest, QueryBlockResponse, BatchIndexBlocksRequest, BatchIndexBlocksResponse
};
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use sv::transaction::Transaction;
use sv::block::Block;
use sv::util::{deserialize, serialize, hash::Sha256d};
use sled::Db;
use hex;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use tracing::{info, warn};
use shared::ShardManager;
use toml;

tonic::include_proto!("index");
tonic::include_proto!("auth");

#[derive(Debug)]
struct IndexServiceImpl {
    tx_db: Arc<Db>,
    block_db: Arc<Db>,
    auth_client: AuthClient<tonic::transport::Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    shard_manager: Arc<ShardManager>,
}

impl IndexServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let tx_db = Arc::new(sled::open(format!("tx_index_db_{}", shard_id)).expect("Failed to open tx index DB"));
        let block_db = Arc::new(sled::open(format!("block_index_db_{}", shard_id)).expect("Failed to open block index DB"));
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("index_requests_total", "Total index requests").unwrap();
        let latency_ms = Gauge::new("index_latency_ms", "Average index request latency").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        let shard_manager = Arc::new(ShardManager::new());

        IndexServiceImpl {
            tx_db,
            block_db,
            auth_client,
            registry,
            requests_total,
            latency_ms,
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
            service: "index_service".to_string(),
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
}

#[tonic::async_trait]
impl Index for IndexServiceImpl {
    async fn index_transaction(&self, request: Request<IndexTransactionRequest>) -> Result<Response<IndexTransactionResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "IndexTransaction").await?;

        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Indexing transaction: {}", req.tx_hex);
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
        let txid = tx.txid().to_string();

        let result = self.tx_db
            .insert(txid.clone(), tx_bytes)
            .map_err(|e| {
                warn!("Failed to index transaction {}: {}", txid, e);
                Status::internal(format!("Failed to index transaction: {}", e))
            })?;
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully indexed transaction: {}", txid);
        Ok(Response::new(IndexTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn query_transaction(&self, request: Request<QueryTransactionRequest>) -> Result<Response<QueryTransactionResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "QueryTransaction").await?;

        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Querying transaction: {}", req.txid);
        let result = self.tx_db
            .get(&req.txid)
            .map_err(|e| {
                warn!("Failed to query transaction {}: {}", req.txid, e);
                Status::internal(format!("Failed to query transaction: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        if let Some(tx_bytes) = result {
            info!("Found transaction: {}", req.txid);
            Ok(Response::new(QueryTransactionResponse {
                tx_hex: hex::encode(tx_bytes),
                error: "".to_string(),
            }))
        } else {
            warn!("Transaction not found: {}", req.txid);
            Ok(Response::new(QueryTransactionResponse {
                tx_hex: "".to_string(),
                error: "Transaction not found".to_string(),
            }))
        }
    }

    async fn batch_index_transactions(&self, request: Request<BatchIndexTransactionsRequest>) -> Result<Response<BatchIndexTransactionsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BatchIndexTransactions").await?;

        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Batch indexing {} transactions", req.tx_hexes.len());
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let tx_bytes = match hex::decode(&tx_hex) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("Invalid tx_hex: {}", e);
                    results.push(IndexTransactionResponse {
                        success: false,
                        error: format!("Invalid tx_hex: {}", e),
                    });
                    continue;
                }
            };
            let tx: Transaction = match deserialize(&tx_bytes) {
                Ok(tx) => tx,
                Err(e) => {
                    warn!("Invalid transaction: {}", e);
                    results.push(IndexTransactionResponse {
                        success: false,
                        error: format!("Invalid transaction: {}", e),
                    });
                    continue;
                }
            };
            let txid = tx.txid().to_string();
            match self.tx_db.insert(txid.clone(), tx_bytes) {
                Ok(_) => {
                    results.push(IndexTransactionResponse {
                        success: true,
                        error: "".to_string(),
                    });
                    info!("Successfully indexed transaction: {}", txid);
                }
                Err(e) => {
                    warn!("Failed to index transaction {}: {}", txid, e);
                    results.push(IndexTransactionResponse {
                        success: false,
                        error: format!("Failed to index transaction: {}", e),
                    });
                }
            }
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Completed batch indexing {} transactions", results.len());
        Ok(Response::new(BatchIndexTransactionsResponse { results }))
    }

    async fn index_block(&self, request: Request<IndexBlockRequest>) -> Result<Response<IndexBlockResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "IndexBlock").await?;

        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Indexing block: {}", req.block_hex);
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| {
                warn!("Invalid block_hex: {}", e);
                Status::invalid_argument(format!("Invalid block_hex: {}", e))
            })?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| {
                warn!("Invalid block: {}", e);
                Status::invalid_argument(format!("Invalid block: {}", e))
            })?;
        let block_hash = block.header.hash().to_string();

        let result = self.block_db
            .insert(block_hash.clone(), block_bytes)
            .map_err(|e| {
                warn!("Failed to index block {}: {}", block_hash, e);
                Status::internal(format!("Failed to index block: {}", e))
            })?;
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully indexed block: {}", block_hash);
        Ok(Response::new(IndexBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn query_block(&self, request: Request<QueryBlockRequest>) -> Result<Response<QueryBlockResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "QueryBlock").await?;

        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Querying block: {}", req.block_hash);
        let result = self.block_db
            .get(&req.block_hash)
            .map_err(|e| {
                warn!("Failed to query block {}: {}", req.block_hash, e);
                Status::internal(format!("Failed to query block: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        if let Some(block_bytes) = result {
            info!("Found block: {}", req.block_hash);
            Ok(Response::new(QueryBlockResponse {
                block_hex: hex::encode(block_bytes),
                error: "".to_string(),
            }))
        } else {
            warn!("Block not found: {}", req.block_hash);
            Ok(Response::new(QueryBlockResponse {
                block_hex: "".to_string(),
                error: "Block not found".to_string(),
            }))
        }
    }

    async fn batch_index_blocks(&self, request: Request<BatchIndexBlocksRequest>) -> Result<Response<BatchIndexBlocksResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BatchIndexBlocks").await?;

        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Batch indexing {} blocks", req.block_hexes.len());
        let mut results = vec![];

        for block_hex in req.block_hexes {
            let block_bytes = match hex::decode(&block_hex) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!("Invalid block_hex: {}", e);
                    results.push(IndexBlockResponse {
                        success: false,
                        error: format!("Invalid block_hex: {}", e),
                    });
                    continue;
                }
            };
            let block: Block = match deserialize(&block_bytes) {
                Ok(block) => block,
                Err(e) => {
                    warn!("Invalid block: {}", e);
                    results.push(IndexBlockResponse {
                        success: false,
                        error: format!("Invalid block: {}", e),
                    });
                    continue;
                }
            };
            let block_hash = block.header.hash().to_string();
            match self.block_db.insert(block_hash.clone(), block_bytes) {
                Ok(_) => {
                    results.push(IndexBlockResponse {
                        success: true,
                        error: "".to_string(),
                    });
                    info!("Successfully indexed block: {}", block_hash);
                }
                Err(e) => {
                    warn!("Failed to index block {}: {}", block_hash, e);
                    results.push(IndexBlockResponse {
                        success: false,
                        error: format!("Failed to index block: {}", e),
                    });
                }
            }
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Completed batch indexing {} blocks", results.len());
        Ok(Response::new(BatchIndexBlocksResponse { results }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "[::1]:50062".parse().unwrap();
    let index_service = IndexServiceImpl::new().await;

    println!("Index service listening on {}", addr);

    Server::builder()
        .add_service(IndexServer::new(index_service))
        .serve(addr)
        .await?;

    Ok(())
}
