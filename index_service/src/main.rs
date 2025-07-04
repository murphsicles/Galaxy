use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use governor::{Quota, RateLimiter};
use index::index_server::{Index, IndexServer};
use index::{
    GetMetricsRequest, GetMetricsResponse, IndexTransactionRequest, IndexTransactionResponse,
    QueryBlockRequest, QueryBlockResponse, QueryTransactionRequest, QueryTransactionResponse,
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
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};
use tracing::{info, warn};

tonic::include_proto!("index");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct IndexServiceImpl {
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    db: Arc<Mutex<Db>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    index_throughput: Gauge,
    rate_limiter:
        Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl IndexServiceImpl {
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
            sled::open("index_db").expect("Failed to open sled database"),
        ));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("index_requests_total", "Total index requests").unwrap();
        let latency_ms = Gauge::new("index_latency_ms", "Average index request latency").unwrap();
        let alert_count = Counter::new("index_alert_count", "Total alerts sent").unwrap();
        let index_throughput =
            Gauge::new("index_throughput", "Indexed transactions per second").unwrap();
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

        IndexServiceImpl {
            auth_client,
            alert_client,
            db,
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
            service: "index_service".to_string(),
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
impl Index for IndexServiceImpl {
    async fn query_transaction(
        &self,
        request: Request<QueryTransactionRequest>,
    ) -> Result<Response<QueryTransactionResponse>, Status> {
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
        self.authorize(&user_id, "QueryTransaction").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let db = self.db.lock().await;
        let key = format!("tx:{}", req.txid);

        let tx_hex = match db.get(key.as_bytes()) {
            Ok(Some(value)) => {
                let tx: Transaction = deserialize(&value).map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    let _ = self
                        .send_alert(
                            "query_tx_invalid_deserialization",
                            &format!("Invalid transaction: {}", e),
                            2,
                        )
                        .await;
                    Status::internal(format!("Invalid transaction: {}", e))
                })?;
                hex::encode(serialize(&tx))
            }
            Ok(None) => {
                warn!("Transaction not found: {}", req.txid);
                let _ = self
                    .send_alert(
                        "query_tx_not_found",
                        &format!("Transaction not found: {}", req.txid),
                        2,
                    )
                    .await;
                return Ok(Response::new(QueryTransactionResponse {
                    success: false,
                    tx_hex: "".to_string(),
                    error: "Transaction not found".to_string(),
                }));
            }
            Err(e) => {
                warn!("Failed to query transaction: {}", e);
                let _ = self
                    .send_alert(
                        "query_tx_failed",
                        &format!("Failed to query transaction: {}", e),
                        2,
                    )
                    .await;
                return Ok(Response::new(QueryTransactionResponse {
                    success: false,
                    tx_hex: "".to_string(),
                    error: format!("Failed to query transaction: {}", e),
                }));
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(QueryTransactionResponse {
            success: true,
            tx_hex,
            error: "".to_string(),
        }))
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
                    "index_tx_invalid_hex",
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
                    "index_tx_invalid_deserialization",
                    &format!("Invalid transaction: {}", e),
                    2,
                )
                .await;
            Status::invalid_argument(format!("Invalid transaction: {}", e))
        })?;

        let db = self.db.lock().await;
        let key = format!("tx:{}", tx.txid());
        db.insert(key.as_bytes(), serialize(&tx)).map_err(|e| {
            warn!("Failed to index transaction: {}", e);
            let _ = self
                .send_alert(
                    "index_tx_failed",
                    &format!("Failed to index transaction: {}", e),
                    2,
                )
                .await;
            Status::internal(format!("Failed to index transaction: {}", e))
        })?;

        self.index_throughput.set(1.0); // Placeholder
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(IndexTransactionResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn query_block(
        &self,
        request: Request<QueryBlockRequest>,
    ) -> Result<Response<QueryBlockResponse>, Status> {
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
        self.authorize(&user_id, "QueryBlock").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let db = self.db.lock().await;
        let key = format!("block:{}", req.block_hash);

        let block_hex = match db.get(key.as_bytes()) {
            Ok(Some(value)) => {
                let block: Block = deserialize(&value).map_err(|e| {
                    warn!("Invalid block: {}", e);
                    let _ = self
                        .send_alert(
                            "query_block_invalid_deserialization",
                            &format!("Invalid block: {}", e),
                            2,
                        )
                        .await;
                    Status::internal(format!("Invalid block: {}", e))
                })?;
                hex::encode(serialize(&block))
            }
            Ok(None) => {
                warn!("Block not found: {}", req.block_hash);
                let _ = self
                    .send_alert(
                        "query_block_not_found",
                        &format!("Block not found: {}", req.block_hash),
                        2,
                    )
                    .await;
                return Ok(Response::new(QueryBlockResponse {
                    success: false,
                    block_hex: "".to_string(),
                    error: "Block not found".to_string(),
                }));
            }
            Err(e) => {
                warn!("Failed to query block: {}", e);
                let _ = self
                    .send_alert(
                        "query_block_failed",
                        &format!("Failed to query block: {}", e),
                        2,
                    )
                    .await;
                return Ok(Response::new(QueryBlockResponse {
                    success: false,
                    block_hex: "".to_string(),
                    error: format!("Failed to query block: {}", e),
                }));
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(QueryBlockResponse {
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

        self.requests_total.inc();
        let start = std::time::Instant::now();

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(GetMetricsResponse {
            service_name: "index_service".to_string(),
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

    let addr = "[::1]:50059".parse().unwrap();
    let index_service = IndexServiceImpl::new().await;

    println!("Index service listening on {}", addr);

    Server::builder()
        .add_service(IndexServer::new(index_service))
        .serve(addr)
        .await?;

    Ok(())
}
