use alert::alert_client::AlertClient;
use alert::SendAlertRequest;
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use governor::{Quota, RateLimiter};
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use storage::storage_server::{Storage, StorageServer};
use storage::{
    AddUtxoRequest, AddUtxoResponse, BatchAddUtxoRequest, BatchAddUtxoResponse, GetMetricsRequest,
    GetMetricsResponse, QueryUtxoRequest, QueryUtxoResponse, RemoveUtxoRequest, RemoveUtxoResponse,
};
use tigerbeetle_unofficial as tigerbeetle;
use tokio::sync::Mutex;
use toml;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};
use tracing::{info, warn};

tonic::include_proto!("storage");
tonic::include_proto!("auth");
tonic::include_proto!("alert");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct StorageServiceImpl {
    utxos: Arc<Mutex<HashMap<String, (String, u64)>>>,
    auth_client: AuthClient<Channel>,
    alert_client: AlertClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    rate_limiter:
        Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl StorageServiceImpl {
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
        let utxos = Arc::new(Mutex::new(HashMap::new()));
        let registry = Arc::new(Registry::new());
        let requests_total =
            Counter::new("storage_requests_total", "Total storage requests").unwrap();
        let latency_ms =
            Gauge::new("storage_latency_ms", "Average storage request latency").unwrap();
        let alert_count = Counter::new("storage_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        StorageServiceImpl {
            utxos,
            auth_client,
            alert_client,
            registry,
            requests_total,
            latency_ms,
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
            service: "storage_service".to_string(),
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
impl Storage for StorageServiceImpl {
    async fn query_utxo(
        &self,
        request: Request<QueryUtxoRequest>,
    ) -> Result<Response<QueryUtxoResponse>, Status> {
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
        self.authorize(&user_id, "QueryUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let utxos = self.utxos.lock().await;
        let exists = utxos.contains_key(&req.txid);

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(QueryUtxoResponse {
            exists,
            script_pubkey: utxos
                .get(&req.txid)
                .map(|(script, _)| script.clone())
                .unwrap_or_default(),
            amount: utxos.get(&req.txid).map(|(_, amount)| *amount).unwrap_or(0),
            error: if exists {
                "".to_string()
            } else {
                "UTXO not found".to_string()
            },
        }))
    }

    async fn add_utxo(
        &self,
        request: Request<AddUtxoRequest>,
    ) -> Result<Response<AddUtxoResponse>, Status> {
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
        self.authorize(&user_id, "AddUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut utxos = self.utxos.lock().await;
        utxos.insert(req.txid.clone(), (req.script_pubkey.clone(), req.amount));

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(AddUtxoResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn remove_utxo(
        &self,
        request: Request<RemoveUtxoRequest>,
    ) -> Result<Response<RemoveUtxoResponse>, Status> {
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
        self.authorize(&user_id, "RemoveUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut utxos = self.utxos.lock().await;
        let removed = utxos.remove(&req.txid).is_some();

        if !removed {
            warn!("UTXO not found for txid: {}", req.txid);
            let _ = self
                .send_alert(
                    "remove_utxo_not_found",
                    &format!("UTXO not found for txid: {}", req.txid),
                    2,
                )
                .await;
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(RemoveUtxoResponse {
            success: removed,
            error: if removed {
                "".to_string()
            } else {
                "UTXO not found".to_string()
            },
        }))
    }

    async fn batch_add_utxo(
        &self,
        request: Request<BatchAddUtxoRequest>,
    ) -> Result<Response<BatchAddUtxoResponse>, Status> {
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
        self.authorize(&user_id, "BatchAddUtxo").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let mut utxos = self.utxos.lock().await;
        let mut results = vec![];

        for utxo in req.utxos {
            utxos.insert(utxo.txid.clone(), (utxo.script_pubkey.clone(), utxo.amount));
            results.push(AddUtxoResponse {
                success: true,
                error: "".to_string(),
            });
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchAddUtxoResponse { results }))
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
            service_name: "storage_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: 0,   // Not applicable
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

    let addr = "[::1]:50053".parse().unwrap();
    let storage_service = StorageServiceImpl::new().await;

    println!("Storage service listening on {}", addr);

    Server::builder()
        .add_service(StorageServer::new(storage_service))
        .serve(addr)
        .await?;

    Ok(())
}
