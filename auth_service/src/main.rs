use tonic::{transport::Server, Request, Response, Status};
use auth::auth_server::{Auth, AuthServer};
use auth::{
    AuthenticateRequest, AuthenticateResponse, AuthorizeRequest, AuthorizeResponse,
    GetMetricsRequest, GetMetricsResponse,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use prometheus::{Counter, Gauge, Registry};
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use tracing::{info, warn};
use shared::ShardManager;
use toml;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;

tonic::include_proto!("auth");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct AuthServiceImpl {
    users: Arc<Mutex<HashMap<String, String>>>,
    roles: Arc<Mutex<HashMap<String, Vec<String>>>>,
    secret_key: EncodingKey,
    decoding_key: DecodingKey,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl AuthServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"]
            .as_integer()
            .unwrap_or(0) as u32;
        let jwt_secret = config["auth"]["jwt_secret"]
            .as_str()
            .unwrap_or("secret")
            .to_string();

        let users = Arc::new(Mutex::new(HashMap::new()));
        let roles = Arc::new(Mutex::new(HashMap::new()));
        let secret_key = EncodingKey::from_secret(jwt_secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(jwt_secret.as_bytes());
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("auth_requests_total", "Total auth requests").unwrap();
        let latency_ms = Gauge::new("auth_latency_ms", "Average auth request latency").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(
            Quota::per_second(NonZeroU32::new(1000).unwrap()),
        ));
        let shard_manager = Arc::new(ShardManager::new());

        AuthServiceImpl {
            users,
            roles,
            secret_key,
            decoding_key,
            registry,
            requests_total,
            latency_ms,
            rate_limiter,
            shard_manager,
        }
    }
}

#[tonic::async_trait]
impl Auth for AuthServiceImpl {
    async fn authenticate(
        &self,
        request: Request<AuthenticateRequest>,
    ) -> Result<Response<AuthenticateResponse>, Status> {
        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let claims = decode::<HashMap<String, String>>(
            &req.token,
            &self.decoding_key,
            &Validation::new(jsonwebtoken::Algorithm::HS256),
        )
        .map_err(|e| {
            warn!("Invalid token: {}", e);
            Status::unauthenticated(format!("Invalid token: {}", e))
        })?;

        let user_id = claims
            .claims
            .get("sub")
            .cloned()
            .ok_or_else(|| Status::unauthenticated("Missing user ID in token"))?;
        let mut users = self.users.lock().await;
        users.insert(user_id.clone(), req.token.clone());

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(AuthenticateResponse {
            success: true,
            user_id,
            error: "".to_string(),
        }))
    }

    async fn authorize(
        &self,
        request: Request<AuthorizeRequest>,
    ) -> Result<Response<AuthorizeResponse>, Status> {
        self.requests_total.inc();
        let start = std::time::Instant::now();
        let req = request.into_inner();
        let roles = self.roles.lock().await;
        let allowed = roles
            .get(&req.user_id)
            .map(|user_roles| {
                user_roles.contains(&format!("{}:{}", req.service, req.method))
                    || user_roles.contains(&format!("{}:*", req.service))
                    || user_roles.contains(&"*:*".to_string())
            })
            .unwrap_or(false);

        if !allowed {
            warn!(
                "Authorization denied for user: {}, service: {}, method: {}",
                req.user_id, req.service, req.method
            );
            return Ok(Response::new(AuthorizeResponse {
                allowed: false,
                error: "Permission denied".to_string(),
            }));
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(AuthorizeResponse {
            allowed: true,
            error: "".to_string(),
        }))
    }

    async fn get_metrics(
        &self,
        request: Request<GetMetricsRequest>,
    ) -> Result<Response<GetMetricsResponse>, Status> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(GetMetricsResponse {
            service_name: "auth_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
            cache_hits: 0, // Not applicable
            alert_count: 0, // Not applicable
            index_throughput: 0.0, // Not applicable
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "[::1]:50060".parse().unwrap();
    let auth_service = AuthServiceImpl::new().await;

    println!("Auth service listening on {}", addr);

    Server::builder()
        .add_service(AuthServer::new(auth_service))
        .serve(addr)
        .await?;

    Ok(())
}
