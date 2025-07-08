use shared::alert::alert_server::{Alert, AlertServer};
use shared::alert::{
    SendAlertRequest, SendAlertResponse, SubscribeToAlertsRequest, SubscribeToAlertsResponse,
};
use async_stream::try_stream;
use shared::auth::auth_client::AuthClient;
use shared::auth::{AuthenticateRequest, AuthorizeRequest};
use governor::{Quota, RateLimiter};
use prometheus::{Counter, Gauge, Registry};
use shared::ShardManager;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::Stream;
use toml;
use tonic::{transport::Server, Request, Response, Status, Streaming};
use tracing::{info, warn};

#[derive(Debug)]
struct AlertServiceImpl {
    auth_client: AuthClient<tonic::transport::Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl AlertServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("alert_requests_total", "Total alert requests").unwrap();
        let latency_ms = Gauge::new("alert_latency_ms", "Average alert request latency").unwrap();
        let alert_count = Counter::new("alert_alert_count", "Total alerts sent").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));
        let shard_manager = Arc::new(ShardManager::new());

        AlertServiceImpl {
            auth_client,
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
            service: "alert_service".to_string(),
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
}

#[tonic::async_trait]
impl Alert for AlertServiceImpl {
    async fn send_alert(
        &self,
        request: Request<SendAlertRequest>,
    ) -> Result<Response<SendAlertResponse>, Status> {
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
        self.authorize(&user_id, "SendAlert").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        self.alert_count.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();

        info!(
            "Sending alert: type={}, message={}, severity={}",
            req.event_type, req.message, req.severity
        );

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(SendAlertResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    type SubscribeToAlertsStream = Pin<Box<dyn Stream<Item = Result<SubscribeToAlertsResponse, Status>> + Send>>;

    async fn subscribe_to_alerts(
        &self,
        request: Request<Streaming<SubscribeToAlertsRequest>>,
    ) -> Result<Response<Self::SubscribeToAlertsStream>, Status> {
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
        self.authorize(&user_id, "SubscribeToAlerts").await?;
        self.rate_limiter.until_ready().await;

        self.requests_total.inc();
        let start = std::time::Instant::now();
        let mut stream = request.into_inner();
        let requests_total = self.requests_total.clone();
        let latency_ms = self.latency_ms.clone();
        let alert_count = self.alert_count.clone();

        let output = try_stream! {
            while let Some(req) = stream.next().await {
                requests_total.inc();
                let start_inner = std::time::Instant::now();
                info!("Received alert subscription request: type={}", req.event_type);
                alert_count.inc();
                yield SubscribeToAlertsResponse {
                    event_type: req.event_type,
                    message: "Subscribed to alerts".to_string(),
                    severity: 1,
                    error: "".to_string(),
                };
                latency_ms.set(start_inner.elapsed().as_secs_f64() * 1000.0);
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(output)))
    }

    async fn get_metrics(
        &self,
        request: Request<shared::metrics::GetMetricsRequest>,
    ) -> Result<Response<shared::metrics::GetMetricsResponse>, Status> {
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
        Ok(Response::new(shared::metrics::GetMetricsResponse {
            service_name: "alert_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0,
            cache_hits: 0,
            alert_count: self.alert_count.get() as u64,
            index_throughput: 0.0,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "[::1]:50061".parse().unwrap();
    let alert_service = AlertServiceImpl::new().await;

    println!("Alert service listening on {}", addr);

    Server::builder()
        .add_service(AlertServer::new(alert_service))
        .serve(addr)
        .await?;

    Ok(())
}
