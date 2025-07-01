use tonic::{transport::Server, Request, Response, Status};
use storage::storage_server::{Storage, StorageServer};
use storage::{
    QueryUtxoRequest, QueryUtxoResponse, AddUtxoRequest, AddUtxoResponse,
    RemoveUtxoRequest, RemoveUtxoResponse, BatchAddUtxoRequest, BatchAddUtxoResponse
};
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use metrics::metrics_client::MetricsClient;
use metrics::{GetMetricsRequest, GetMetricsResponse};
use tigerbeetle::client::{Client, Config};
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

tonic::include_proto!("storage");
tonic::include_proto!("auth");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct StorageServiceImpl {
    utxo_db: Arc<Mutex<HashMap<(String, u32), (String, u64)>>>, // Fallback for testing
    tb_client: Option<Client>, // Tiger Beetle client
    auth_client: AuthClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl StorageServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let tb_address = config["testnet"]["tigerbeetle_address"]
            .as_str()
            .unwrap()
            .to_string();
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let tb_client = Client::new(Config {
            cluster_id: 0,
            replica_id: 0,
            addresses: vec![tb_address],
            keep_alive: true, // Enable connection keep-alive
        })
        .await
        .ok();
        let utxo_db = Arc::new(Mutex::new(HashMap::new()));
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("storage_requests_total", "Total storage requests").unwrap();
        let latency_ms = Gauge::new("storage_latency_ms", "Average storage request latency").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(1000).unwrap())));
        let shard_manager = Arc::new(ShardManager::new());

        StorageServiceImpl {
            utxo_db,
            tb_client,
            auth_client,
            registry,
            requests_total,
            latency_ms,
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
            service: "storage_service".to_string(),
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

    async fn query_utxo_tb(&self, txid: &str, vout: u32) -> Result<Option<(String, u64)>, String> {
        info!("Querying UTXO in Tiger Beetle: {}:{}", txid, vout);
        Ok(None) // Placeholder for real Tiger Beetle query
    }

    async fn add_utxo_tb(&self, txid: &str, vout: u32, script_pubkey: &str, amount: u64) -> Result<(), String> {
        info!("Adding UTXO to Tiger Beetle: {}:{}", txid, vout);
        Ok(()) // Placeholder for real Tiger Beetle write
    }

    async fn remove_utxo_tb(&self, txid: &str, vout: u32) -> Result<(), String> {
        info!("Removing UTXO from Tiger Beetle: {}:{}", txid, vout);
        Ok(()) // Placeholder for real Tiger Beetle delete
    }

    async fn batch_add_utxo_tb(&self, utxos: Vec<(String, u32, String, u64)>) -> Result<Vec<bool>, String> {
        info!("Batch adding {} UTXOs to Tiger Beetle", utxos.len());
        Ok(vec![true; utxos.len()]) // Placeholder for real Tiger Beetle batch write
    }
}

#[tonic::async_trait]
impl Storage for StorageServiceImpl {
    async fn query_utxo(&self, request: Request<QueryUtxoRequest>) -> Result<Response<QueryUtxoResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "QueryUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Querying UTXO: {}:{}", request.get_ref().txid, request.get_ref().vout);
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            match self.query_utxo_tb(&req.txid, req.vout).await {
                Ok(Some((script_pubkey, amount))) => {
                    self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                    info!("Found UTXO: {}:{}", req.txid, req.vout);
                    return Ok(Response::new(QueryUtxoResponse {
                        exists: true,
                        script_pubkey,
                        amount,
                        error: "".to_string(),
                    }));
                }
                Ok(None) => {
                    self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                    warn!("UTXO not found: {}:{}", req.txid, req.vout);
                    return Ok(Response::new(QueryUtxoResponse {
                        exists: false,
                        script_pubkey: "".to_string(),
                        amount: 0,
                        error: "UTXO not found".to_string(),
                    }));
                }
                Err(e) => {
                    self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                    warn!("UTXO query failed: {}", e);
                    return Ok(Response::new(QueryUtxoResponse {
                        exists: false,
                        script_pubkey: "".to_string(),
                        amount: 0,
                        error: e,
                    }));
                }
            }
        }

        let utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);
        let result = if let Some((script_pubkey, amount)) = utxo_db.get(&key) {
            QueryUtxoResponse {
                exists: true,
                script_pubkey: script_pubkey.clone(),
                amount: *amount,
                error: "".to_string(),
            }
        } else {
            QueryUtxoResponse {
                exists: false,
                script_pubkey: "".to_string(),
                amount: 0,
                error: "UTXO not found".to_string(),
            }
        };
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Queried UTXO {}: {}:{}", if result.exists { "found" } else { "not found" }, key.0, key.1);
        Ok(Response::new(result))
    }

    async fn add_utxo(&self, request: Request<AddUtxoRequest>) -> Result<Response<AddUtxoResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "AddUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Adding UTXO: {}:{}", request.get_ref().txid, request.get_ref().vout);
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            if let Err(e) = self.add_utxo_tb(&req.txid, req.vout, &req.script_pubkey, req.amount).await {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                warn!("Failed to add UTXO: {}", e);
                return Ok(Response::new(AddUtxoResponse {
                    success: false,
                    error: e,
                }));
            }
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            info!("Successfully added UTXO to Tiger Beetle: {}:{}", req.txid, req.vout);
            return Ok(Response::new(AddUtxoResponse {
                success: true,
                error: "".to_string(),
            }));
        }

        let mut utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);
        utxo_db.insert(key, (req.script_pubkey, req.amount));
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully added UTXO to HashMap: {}:{}", key.0, key.1);
        Ok(Response::new(AddUtxoResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn remove_utxo(&self, request: Request<RemoveUtxoRequest>) -> Result<Response<RemoveUtxoResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "RemoveUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Removing UTXO: {}:{}", request.get_ref().txid, request.get_ref().vout);
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            if let Err(e) = self.remove_utxo_tb(&req.txid, req.vout).await {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                warn!("Failed to remove UTXO: {}", e);
                return Ok(Response::new(RemoveUtxoResponse {
                    success: false,
                    error: e,
                }));
            }
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            info!("Successfully removed UTXO from Tiger Beetle: {}:{}", req.txid, req.vout);
            return Ok(Response::new(RemoveUtxoResponse {
                success: true,
                error: "".to_string(),
            }));
        }

        let mut utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);
        let success = utxo_db.remove(&key).is_some();
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Remove UTXO {}: {}:{}", if success { "succeeded" } else { "failed" }, key.0, key.1);
        Ok(Response::new(RemoveUtxoResponse {
            success,
            error: if success { "".to_string() } else { "UTXO not found".to_string() },
        }))
    }

    async fn batch_add_utxo(&self, request: Request<BatchAddUtxoRequest>) -> Result<Response<BatchAddUtxoResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "BatchAddUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = Instant::now();
        info!("Batch adding {} UTXOs", request.get_ref().utxos.len());
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            let utxos = req
                .utxos
                .into_iter()
                .map(|u| (u.txid, u.vout, u.script_pubkey, u.amount))
                .collect();
            let results = self
                .batch_add_utxo_tb(utxos)
                .await
                .map_err(|e| {
                    warn!("Batch add failed: {}", e);
                    Status::internal(format!("Batch add failed: {}", e))
                })?;
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            info!("Successfully batch added {} UTXOs to Tiger Beetle", results.len());
            return Ok(Response::new(BatchAddUtxoResponse {
                results: results
                    .into_iter()
                    .map(|success| AddUtxoResponse {
                        success,
                        error: if success { "".to_string() } else { "Failed to add UTXO".to_string() },
                    })
                    .collect(),
            }));
        }

        let mut utxo_db = self.utxo_db.lock().await;
        let mut results = vec![];
        for utxo in req.utxos {
            let key = (utxo.txid, utxo.vout);
            utxo_db.insert(key, (utxo.script_pubkey, utxo.amount));
            results.push(AddUtxoResponse {
                success: true,
                error: "".to_string(),
            });
        }
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Successfully batch added {} UTXOs to HashMap", results.len());
        Ok(Response::new(BatchAddUtxoResponse { results }))
    }

    async fn get_metrics(&self, request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "GetMetrics").await?;

        Ok(Response::new(GetMetricsResponse {
            service_name: "storage_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "[::1]:50053".parse().unwrap();
    let storage_service = StorageServiceImpl::new().await;

    println!("Storage service listening on {}", addr);

    Server::builder()
        .add_service(StorageServer::new(storage_service))
        .serve(addr)
        .await?;

    Ok(())
        }
