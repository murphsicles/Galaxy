use tonic::{transport::{Server, Channel}, Request, Response, Status};
use index::index_server::{Index, IndexServer};
use index::{IndexTransactionRequest, IndexTransactionResponse, IndexBlockRequest, IndexBlockResponse, GetMetricsRequest, GetMetricsResponse};
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use sv::transaction::Transaction;
use sv::block::Block;
use sv::util::{deserialize, serialize};
use sled::Db;
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

tonic::include_proto!("index");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct IndexServiceImpl {
    db: Arc<Db>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    index_throughput: Gauge,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl IndexServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let db = Arc::new(sled::open(format!("index_db_{}", shard_id)).expect("Failed to open sled DB"));
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let alert_client = AlertClient::connect("http://[::1]:50061")
            .await
            .expect("Failed to connect to alert_service");
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("index_requests_total", "Total index requests").unwrap();
        let latency_ms = Gauge::new("index_latency_ms", "Average index request latency").unwrap();
        let alert_count = Counter::new("index_alert_count", "Total alerts sent").unwrap();
        let index_throughput = Gauge::new("index_index_throughput", "Indexed items per second").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(index_throughput.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));
        let shard_manager = Arc::new(ShardManager::new());

        IndexServiceImpl {
            db,
            auth_client,
            alert_client,
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
}

#[tonic::async_trait]
impl Index for IndexServiceImpl {
    async fn index_transaction(&self, request: Request<IndexTransactionRequest>) -> Result<Response<IndexTransactionResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "IndexTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Indexing transaction: {}", request.get_ref().tx_hex);
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| {
                warn!("Invalid tx_hex: {}", e);
                let _ = self.send_alert("index_tx_invalid_format", &format!("Invalid tx_hex: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid tx_hex: {}", e))
            })?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| {
                warn!("Invalid transaction: {}", e);
                let _ = self.send_alert("index_tx_invalid_format", &format!("Invalid transaction: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid transaction: {}", e))
            })?;

        let txid = tx.txid().to_string();
        self.db
            .insert(format!("tx:{}", txid), tx_bytes)
            .map_err(|e| {
                warn!("Failed to index transaction {}: {}", txid, e);
                let _ = self.send_alert("index_tx_failed", &format!("Failed to index transaction {}: {}", txid, e), 2).await;
                Status::internal(format!("Failed to index transaction: {}", e))
            })?;

        self.index_throughput.inc_by(1.0);
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully indexed transaction: {}", txid);
        Ok(Response::new(IndexTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn index_block(&self, request: Request<IndexBlockRequest>) -> Result<Response<IndexBlockResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "IndexBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Indexing block: {}", request.get_ref().block_hex);
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| {
                warn!("Invalid block_hex: {}", e);
                let _ = self.send_alert("index_block_invalid_format", &format!("Invalid block_hex: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid block_hex: {}", e))
            })?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| {
                warn!("Invalid block: {}", e);
                let _ = self.send_alert("index_block_invalid_format", &format!("Invalid block: {}", e), 2).await;
                Status::invalid_argument(format!("Invalid block: {}", e))
            })?;

        let block_hash = block.header.hash().to_string();
        self.db
            .insert(format!("block:{}", block_hash), block_bytes)
            .map_err(|e| {
                warn!("Failed to index block {}: {}", block_hash, e);
                let _ = self.send_alert("index_block_failed", &format!("Failed to index block {}: {}", block_hash, e), 2).await;
                Status::internal(format!("Failed to index block: {}", e))
            })?;

        self.index_throughput.inc_by(1.0);
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully indexed block: {}", block_hash);
        Ok(Response::new(IndexBlockResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_metrics(&self, request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetMetrics").await?;

        Ok(Response::new(GetMetricsResponse {
            service_name: "index_service".to_string(),
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

    let addr = "[::1]:50062".parse().unwrap();
    let index_service = IndexServiceImpl::new().await;

    println!("Index service listening on {}", addr);

    Server::builder()
        .add_service(IndexServer::new(index_service))
        .serve(addr)
        .await?;

    Ok(())
}
