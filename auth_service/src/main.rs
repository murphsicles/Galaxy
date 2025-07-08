use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use prometheus::{Counter, Gauge, Registry};
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::Mutex;
use toml;
use governor::{Quota, RateLimiter};
use shared::ShardManager;

#[derive(Serialize, Deserialize, Debug)]
struct AuthenticateRequest {
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthenticateResponse {
    success: bool,
    user_id: String,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthorizeRequest {
    user_id: String,
    service: String,
    method: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthorizeResponse {
    allowed: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsRequest {}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsResponse {
    service_name: String,
    requests_total: u64,
    avg_latency_ms: f64,
    errors_total: u64,
    cache_hits: u64,
    alert_count: u64,
    index_throughput: f64,
}

#[derive(Serialize, Deserialize, Debug)]
enum AuthRequestType {
    Authenticate { request: AuthenticateRequest },
    Authorize { request: AuthorizeRequest },
    GetMetrics { request: GetMetricsRequest },
}

#[derive(Serialize, Deserialize, Debug)]
enum AuthResponseType {
    Authenticate(AuthenticateResponse),
    Authorize(AuthorizeResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct AuthService {
    users: Arc<Mutex<HashMap<String, String>>>,
    roles: Arc<Mutex<HashMap<String, Vec<String>>>>,
    secret_key: EncodingKey,
    decoding_key: DecodingKey,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl AuthService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;
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
        let errors_total = Counter::new("auth_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        AuthService {
            users,
            roles,
            secret_key,
            decoding_key,
            registry,
            requests_total,
            latency_ms,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
        }
    }

    async fn handle_request(&self, request: AuthRequestType) -> Result<AuthResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            AuthRequestType::Authenticate { request } => {
                let claims = match decode::<HashMap<String, String>>(
                    &request.token,
                    &self.decoding_key,
                    &Validation::new(jsonwebtoken::Algorithm::HS256),
                ) {
                    Ok(claims) => claims,
                    Err(e) => {
                        warn!("Invalid token: {}", e);
                        self.errors_total.inc();
                        return Err(format!("Invalid token: {}", e));
                    }
                };

                let user_id = match claims.claims.get("sub").cloned() {
                    Some(id) => id,
                    None => {
                        self.errors_total.inc();
                        return Err("Missing user ID in token".to_string());
                    }
                };
                let mut users = self.users.lock().await;
                users.insert(user_id.clone(), request.token.clone());

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(AuthResponseType::Authenticate(AuthenticateResponse {
                    success: true,
                    user_id,
                    error: "".to_string(),
                }))
            }
            AuthRequestType::Authorize { request } => {
                let roles = self.roles.lock().await;
                let allowed = roles
                    .get(&request.user_id)
                    .map(|user_roles| {
                        user_roles.contains(&format!("{}:{}", request.service, request.method))
                            || user_roles.contains(&format!("{}:*", request.service))
                            || user_roles.contains(&"*:*".to_string())
                    })
                    .unwrap_or(false);

                if !allowed {
                    warn!(
                        "Authorization denied for user: {}, service: {}, method: {}",
                        request.user_id, request.service, request.method
                    );
                    self.errors_total.inc();
                    return Ok(AuthResponseType::Authorize(AuthorizeResponse {
                        allowed: false,
                        error: "Permission denied".to_string(),
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(AuthResponseType::Authorize(AuthorizeResponse {
                    allowed: true,
                    error: "".to_string(),
                }))
            }
            AuthRequestType::GetMetrics { request } => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(AuthResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "auth_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: 0,
                    index_throughput: 0.0,
                }))
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Auth service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: AuthRequestType = match deserialize(&buffer[..n]) {
                                    Ok(req) => req,
                                    Err(e) => {
                                        error!("Deserialization error: {}", e);
                                        service.errors_total.inc();
                                        return;
                                    }
                                };
                                match service.handle_request(request).await {
                                    Ok(response) => {
                                        let encoded = serialize(&response).unwrap();
                                        if let Err(e) = stream.write_all(&encoded).await {
                                            error!("Write error: {}", e);
                                            service.errors_total.inc();
                                        }
                                        if let Err(e) = stream.flush().await {
                                            error!("Flush error: {}", e);
                                            service.errors_total.inc();
                                        }
                                    }
                                    Err(e) => {
                                        error!("Request error: {}", e);
                                        service.errors_total.inc();
                                        let response = AuthResponseType::Authenticate(AuthenticateResponse {
                                            success: false,
                                            user_id: "".to_string(),
                                            error: e,
                                        });
                                        let encoded = serialize(&response).unwrap();
                                        if let Err(e) = stream.write_all(&encoded).await {
                                            error!("Write error: {}", e);
                                        }
                                        if let Err(e) = stream.flush().await {
                                            error!("Flush error: {}", e);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Read error: {}", e);
                                service.errors_total.inc();
                            }
                        }
                    });
                }
                Err(e) => error!("Accept error: {}", e),
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "127.0.0.1:50060";
    let auth_service = AuthService::new().await;
    auth_service.run(addr).await?;
    Ok(())
}
