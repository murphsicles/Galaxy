use tonic::{transport::Server, Request, Response, Status, Streaming};
use alert::alert_server::{Alert, AlertServer};
use alert::{SendAlertRequest, SendAlertResponse, SubscribeToAlertsRequest, AlertResponse};
use auth::auth_client::AuthClient;
use auth::{AuthenticateRequest, AuthorizeRequest};
use tokio::sync::mpsc;
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::{Instant, SystemTime};
use prometheus::{Counter, Gauge, Registry};
use tracing::{info, warn};
use toml;

tonic::include_proto!("alert");
tonic::include_proto!("auth");

#[derive(Debug)]
struct AlertServiceImpl {
    auth_client: AuthClient<tonic::transport::Channel>,
    alert_channel: Arc<Mutex<mpsc::Sender<AlertResponse>>>,
    registry: Arc<Registry>,
    alerts_total: Counter,
    latency_ms: Gauge,
}

impl AlertServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let auth_client = AuthClient::connect("http://[::1]:50060")
            .await
            .expect("Failed to connect to auth_service");
        let (tx, _rx) = mpsc::channel(100);
        let alert_channel = Arc::new(Mutex::new(tx));
        let registry = Arc::new(Registry::new());
        let alerts_total = Counter::new("alerts_total", "Total alerts sent").unwrap();
        let latency_ms = Gauge::new("alert_latency_ms", "Average alert processing latency").unwrap();
        registry.register(Box::new(alerts_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();

        AlertServiceImpl {
            auth_client,
            alert_channel,
            registry,
            alerts_total,
            latency_ms,
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
            service: "alert_service".to_string(),
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
impl Alert for AlertServiceImpl {
    async fn send_alert(&self, request: Request<SendAlertRequest>) -> Result<Response<SendAlertResponse>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "SendAlert").await?;

        self.alerts_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Sending alert: type={}, message={}, severity={}", req.event_type, req.message, req.severity);

        let alert = AlertResponse {
            event_type: req.event_type,
            message: req.message,
            severity: req.severity,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };
        let alert_channel = self.alert_channel.lock().await;
        alert_channel
            .send(alert)
            .await
            .map_err(|e| {
                warn!("Failed to send alert: {}", e);
                Status::internal(format!("Failed to send alert: {}", e))
            })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        info!("Alert sent successfully");
        Ok(Response::new(SendAlertResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn subscribe_to_alerts(&self, request: Request<SubscribeToAlertsRequest>) -> Result<Response<Self::SubscribeToAlertsStream>, Status> {
        let token = request.metadata().get("authorization").ok_or_else(|| Status::unauthenticated("Missing token"))?;
        let user_id = self.authenticate(token.to_str().map_err(|e| Status::invalid_argument("Invalid token format"))?).await?;
        self.authorize(&user_id, "SubscribeToAlerts").await?;

        self.alerts_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        info!("Subscribing to alerts with filter: {}", req.event_type);
        let (tx, mut rx) = mpsc::channel(100);
        let alert_channel = Arc::clone(&self.alert_channel);
        let event_type_filter = req.event_type.clone();

        tokio::spawn(async move {
            let mut alert_channel = alert_channel.lock().await;
            while let Ok(alert) = rx.recv().await {
                if event_type_filter.is_empty() || alert.event_type == event_type_filter {
                    if let Err(e) = tx.send(Ok(alert)).await {
                        warn!("Failed to stream alert: {}", e);
                        break;
                    }
                }
            }
        });

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))))
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
