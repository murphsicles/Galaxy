use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bincode::{deserialize, serialize};
use bitcoin_hashes::{sha256d, Hash};
use dashmap::DashMap;
use governor::{Quota, RateLimiter};
use hex;
use jsonwebtoken::{decode, DecodingKey, Validation};
use lazy_static::lazy_static;
use prometheus::{Counter, Gauge, Registry};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sled::Db;
use std::num::NonZeroU32;
use std::sync::Arc;
use sv::merkle::MerklePath;
use sv::transaction::Transaction as SvTx;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tokio_retry::strategy::ExponentialBackoff;
use tokio_retry::Retry;
use toml;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

// Status enums emulating ARC
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum TxStatus {
    Unknown,
    Queued,
    Received,
    Stored,
    AnnouncedToNetwork,
    RequestedByNetwork,
    SentToNetwork,
    SeenInOrphanMempool,
    SeenOnNetwork,
    Mined,
    Confirmed,
    Rejected(String),
}

// Shared cache: txid -> (status, callback_url, extra_info like block_hash)
type Cache = Arc<DashMap<[u8; 32], (TxStatus, Option<String>, Option<String>)>>;

// Ports from README/config
const AUTH_PORT: u16 = 50060;
const TX_PORT: u16 = 50052;
const NETWORK_PORT: u16 = 50051;
const INDEX_PORT: u16 = 50059;
const BLOCK_PORT: u16 = 50054;
const VALIDATION_PORT: u16 = 50057;
const CONSENSUS_PORT: u16 = 50055;
const ALERT_PORT: u16 = 50061;
const STORAGE_PORT: u16 = 50053;

// JWT secret (align with auth_service, placeholder)
lazy_static! {
    static ref JWT_SECRET: String = "secret_key".to_string();  // From torrent_service example
    static ref DECODING_KEY: DecodingKey = DecodingKey::from_secret(JWT_SECRET.as_bytes());
}

// Auth structs (from examples)
#[derive(Serialize, Deserialize, Debug)]
struct AuthRequest {
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AuthResponse {
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

// Alert (from examples)
#[derive(Serialize, Deserialize, Debug)]
struct AlertRequest {
    event_type: String,
    message: String,
    severity: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct AlertResponse {
    success: bool,
    error: String,
}

// Policy struct
#[derive(Serialize, Deserialize, Debug)]
struct Policy {
    min_fee_sat_per_byte: f64,
    max_tx_size: u64,
    // Add from consensus if needed
}

// Consensus message for policy
#[derive(Serialize, Deserialize, Debug)]
struct GetPolicyRequest {}

#[derive(Serialize, Deserialize, Debug)]
struct GetPolicyResponse {
    policy: Policy,
    error: String,
}

// Tx messages (extend from transaction_service patterns)
#[derive(Serialize, Deserialize, Debug)]
enum TxRequestType {
    ValidateTransaction { tx_hex: String, token: String },
    ProcessTransaction { tx_hex: String, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum TxResponseType {
    ValidateTransaction { success: bool, error: String },
    ProcessTransaction { success: bool, error: String },
}

// Network messages (infer for broadcast)
#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxRequest {
    tx_hex: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxResponse {
    success: bool,
    error: String,
}

// Index messages (for status)
#[derive(Serialize, Deserialize, Debug)]
struct GetTxStatusRequest {
    txid: String,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetTxStatusResponse {
    status: TxStatus,
    block_hash: Option<String>,
    block_height: Option<u64>,
    merkle_path: Option<String>,  // Serialized MerklePath
    error: String,
}

// Block messages (for monitor)
#[derive(Serialize, Deserialize, Debug)]
struct GetRecentBlocksRequest {
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetRecentBlocksResponse {
    blocks: Vec<String>,  // Hex blocks
    error: String,
}

// Validation messages (from file)
#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofRequest {
    proof: ProofBundle,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProofBundle {
    pub tx: SvTx,
    pub path: MerklePath,
    pub header: sv::block::Header,
}

// Storage for UTXO check (from transaction_service)
#[derive(Serialize, Deserialize, Debug)]
struct QueryUtxoRequest {
    txid: String,
    vout: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct QueryUtxoResponse {
    exists: bool,
    script_pubkey: String,
    amount: u64,
    error: String,
}

// Submit structs
#[derive(Deserialize)]
struct SubmitTx {
    raw_tx: String,  // Hex
}

#[derive(Serialize)]
struct TxResponse {
    txid: String,
    status: TxStatus,
    extra_info: Option<String>,
}

// Callback
#[derive(Serialize)]
struct Callback {
    txid: String,
    status: TxStatus,
    block_height: Option<u64>,
    block_hash: Option<String>,
    merkle_path: Option<String>,
}

pub struct MerchantService {
    cache: Cache,
    db: Arc<Mutex<Db>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<gov::state::NotKeyed, gov::state::InMemoryState, gov::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
    auth_service_addr: String,
    alert_service_addr: String,
    client: Client,
    auth_token: Arc<Mutex<String>>,
}

impl MerchantService {
    pub async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("merchant_requests_total", "Total requests").unwrap();
        let latency_ms = Gauge::new("merchant_latency_ms", "Avg latency").unwrap();
        let alert_count = Counter::new("merchant_alert_count", "Alerts sent").unwrap();
        let errors_total = Counter::new("merchant_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();

        let rate_limit = config.get("merchant").and_then(|m| m.get("rate_limit").and_then(|v| v.as_integer())).unwrap_or(10000) as u32;
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(NonZeroU32::new(rate_limit).unwrap())));

        let db = Arc::new(Mutex::new(sled::open("merchant_db").unwrap()));

        let auth_token = Arc::new(Mutex::new("initial_token".to_string()));
        let auth_service_addr = "127.0.0.1:50060".to_string();
        let clone_auth_addr = auth_service_addr.clone();
        let clone_token = auth_token.clone();
        tokio::spawn(async move {
            let interval_secs = config.get("merchant").and_then(|m| m.get("auth_token_rotation_interval_secs").and_then(|v| v.as_integer())).unwrap_or(3600) as u64;
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                if let Ok(new_token) = Self::refresh_token(&clone_auth_addr).await {
                    *clone_token.lock().await = new_token;
                    info!("Refreshed merchant auth token");
                } else {
                    error!("Token refresh failed");
                }
            }
        });

        Self {
            cache: Arc::new(DashMap::new()),
            db,
            registry,
            requests_total,
            latency_ms,
            alert_count,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
            auth_service_addr,
            alert_service_addr: "127.0.0.1:50061".to_string(),
            client: Client::new(),
            auth_token,
        }
    }

    async fn refresh_token(auth_addr: &str) -> Result<String, String> {
        // Align with torrent_service JWT claims
        let claims = super::torrent_service::service::Claims {
            sub: "merchant_service".to_string(),
            exp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as usize + 3600,
        };
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::default(), &claims, &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()))
            .map_err(|e| e.to_string())?;

        let mut stream = TcpStream::connect(auth_addr).await.map_err(|e| e.to_string())?;
        let req = AuthRequest { token: token.clone() };
        let encoded = serialize(&req).map_err(|e| e.to_string())?;
        stream.write_all(&encoded).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;

        let mut buf = vec![0; 1024 * 1024];
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        let resp: AuthResponse = deserialize(&buf[..n]).map_err(|e| e.to_string())?;

        if resp.success {
            Ok(token)
        } else {
            Err(resp.error)
        }
    }

    async fn authenticate(&self, token: &str) -> Result<String, String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await.map_err(|e| e.to_string())?;
        let req = AuthRequest { token: token.to_string() };
        let encoded = serialize(&req).map_err(|e| e.to_string())?;
        stream.write_all(&encoded).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;

        let mut buf = vec![0; 1024 * 1024];
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        let resp: AuthResponse = deserialize(&buf[..n]).map_err(|e| e.to_string())?;

        if resp.success {
            Ok(resp.user_id)
        } else {
            Err(resp.error)
        }
    }

    async fn authorize(&self, user_id: &str, method: &str) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await.map_err(|e| e.to_string())?;
        let req = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "merchant_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&req).map_err(|e| e.to_string())?;
        stream.write_all(&encoded).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;

        let mut buf = vec![0; 1024 * 1024];
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        let resp: AuthorizeResponse = deserialize(&buf[..n]).map_err(|e| e.to_string())?;

        if resp.allowed {
            Ok(())
        } else {
            Err(resp.error)
        }
    }

    async fn send_alert(&self, event_type: &str, message: &str, severity: u32) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.alert_service_addr).await.map_err(|e| e.to_string())?;
        let req = AlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let encoded = serialize(&req).map_err(|e| e.to_string())?;
        stream.write_all(&encoded).await.map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;

        let mut buf = vec![0; 1024 * 1024];
        let n = stream.read(&mut buf).await.map_err(|e| e.to_string())?;
        let resp: AlertResponse = deserialize(&buf[..n]).map_err(|e| e.to_string())?;

        if resp.success {
            self.alert_count.inc();
            Ok(())
        } else {
            Err(resp.error)
        }
    }

    // REST router
    fn router(&self) -> Router {
        Router::new()
            .route("/v1/policy", get(Self::get_policy))
            .route("/v1/health", get(Self::get_health))
            .route("/v1/tx", post(Self::submit_tx).with_state(self.cache.clone()))
            .route("/v1/txs", post(Self::submit_batch))
            .route("/v1/txs/chain", post(Self::submit_chain))
            .route("/v1/tx/:txid", get(Self::get_tx_status))
            .layer(middleware::from_fn(Self::auth_middleware))
            .layer(TraceLayer::new_for_http())
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let router = self.router();
        axum::Server::bind(&addr.parse()?)
            .serve(router.into_make_service())
            .await?;
        Ok(())
    }

    // Middleware for Bearer token auth (JWT decode + delegate to auth_service)
    async fn auth_middleware(req: axum::http::Request<axum::body::Body>, next: axum::middleware::Next) -> impl IntoResponse {
        let headers = req.headers();
        let auth_header = headers.get("Authorization").and_then(|h| h.to_str().ok());
        if let Some(header) = auth_header {
            if let Some(token) = header.strip_prefix("Bearer ") {
                if decode::<serde_json::Value>(token, &*DECODING_KEY, &Validation::default()).is_ok() {
                    return next.run(req).await;
                }
            }
        }
        StatusCode::UNAUTHORIZED.into_response()
    }

    // GET /v1/policy
    async fn get_policy(State(cache): State<Cache>) -> impl IntoResponse {
        // Delegate to consensus
        let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", CONSENSUS_PORT)).await {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let req = GetPolicyRequest {};
        let encoded = match serialize(&req) {
            Ok(e) => e,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if stream.write_all(&encoded).await.is_err() || stream.flush().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0; 1024 * 1024];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let resp: GetPolicyResponse = match deserialize(&buf[..n]) {
            Ok(r) => r,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if !resp.error.is_empty() {
            return (StatusCode::BAD_REQUEST, resp.error).into_response();
        }
        Json(resp.policy)
    }

    // POST /v1/tx
    async fn submit_tx(State(cache): State<Cache>, headers: HeaderMap, Json(body): Json<SubmitTx>) -> impl IntoResponse {
        let tx_bytes = match hex::decode(&body.raw_tx) {
            Ok(b) => b,
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        };
        let tx: SvTx = match sv::util::deserialize(&tx_bytes) {
            Ok(t) => t,
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        };
        let txid_array = tx.txid().to_byte_array();
        let txid_str = hex::encode(txid_array);
        let callback_url = headers.get("X-CallbackUrl").and_then(|v| v.to_str().ok()).map(String::from);

        // Validate via transaction_service
        let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", TX_PORT)).await {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let req = TxRequestType::ValidateTransaction { tx_hex: body.raw_tx.clone(), token: "merchant_token".to_string() };  // Use refreshed token
        let encoded = match serialize(&req) {
            Ok(e) => e,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if stream.write_all(&encoded).await.is_err() || stream.flush().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0; 1024 * 1024];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let resp: TxResponseType = match deserialize(&buf[..n]) {
            Ok(r) => r,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if let TxResponseType::ValidateTransaction { success: false, error } = resp {
            return (StatusCode::BAD_REQUEST, Json(TxResponse { txid: txid_str, status: TxStatus::Rejected(error.clone()), extra_info: Some(error) })).into_response();
        }

        // Process/queue via transaction_service
        let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", TX_PORT)).await {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let req = TxRequestType::ProcessTransaction { tx_hex: body.raw_tx.clone(), token: "merchant_token".to_string() };
        let encoded = match serialize(&req) {
            Ok(e) => e,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if stream.write_all(&encoded).await.is_err() || stream.flush().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0; 1024 * 1024];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let resp: TxResponseType = match deserialize(&buf[..n]) {
            Ok(r) => r,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if let TxResponseType::ProcessTransaction { success: false, error } = resp {
            return (StatusCode::BAD_REQUEST, Json(TxResponse { txid: txid_str, status: TxStatus::Rejected(error.clone()), extra_info: Some(error) })).into_response();
        }

        // Broadcast via network_service
        let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", NETWORK_PORT)).await {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let req = BroadcastTxRequest { tx_hex: body.raw_tx, token: "merchant_token".to_string() };
        let encoded = match serialize(&req) {
            Ok(e) => e,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if stream.write_all(&encoded).await.is_err() || stream.flush().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0; 1024 * 1024];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let resp: BroadcastTxResponse = match deserialize(&buf[..n]) {
            Ok(r) => r,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if !resp.success {
            return (StatusCode::BAD_REQUEST, Json(TxResponse { txid: txid_str, status: TxStatus::Rejected(resp.error.clone()), extra_info: Some(resp.error) })).into_response();
        }

        cache.insert(txid_array, (TxStatus::Queued, callback_url, None));
        Json(TxResponse { txid: txid_str, status: TxStatus::Queued, extra_info: None })
    }

    // POST /v1/txs (batch, parallel validation/broadcast)
    async fn submit_batch(State(cache): State<Cache>, headers: HeaderMap, Json(body): Json<Vec<SubmitTx>>) -> impl IntoResponse {
        let mut responses = vec![];
        let futures = body.into_iter().map(|tx| Self::submit_tx(State(cache.clone()), headers.clone(), Json(tx)));
        let results = futures::future::join_all(futures).await;
        for res in results {
            responses.push(res);
        }
        Json(responses)
    }

    // POST /v1/txs/chain (sequential for dependencies)
    async fn submit_chain(State(cache): State<Cache>, headers: HeaderMap, Json(body): Json<Vec<SubmitTx>>) -> impl IntoResponse {
        let mut responses = vec![];
        for tx in body {
            let res = Self::submit_tx(State(cache.clone()), headers.clone(), Json(tx)).await;
            if res.status_code() != StatusCode::OK {
                return res;
            }
            responses.push(res);
        }
        Json(responses)
    }

    // GET /v1/tx/{txid}
    async fn get_tx_status(Path(txid_str): Path<String>, State(cache): State<Cache>) -> impl IntoResponse {
        let txid_array = match hex::decode(&txid_str).map(|b| b.try_into().ok()) {
            Ok(Some(a)) => a,
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
        if let Some((status, _, extra)) = cache.get(&txid_array) {
            return Json(TxResponse { txid: txid_str, status: status.clone(), extra_info: extra.clone() }).into_response();
        }

        // Query index_service
        let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", INDEX_PORT)).await {
            Ok(s) => s,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let req = GetTxStatusRequest { txid: txid_str.clone(), token: "merchant_token".to_string() };
        let encoded = match serialize(&req) {
            Ok(e) => e,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if stream.write_all(&encoded).await.is_err() || stream.flush().await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0; 1024 * 1024];
        let n = match stream.read(&mut buf).await {
            Ok(n) => n,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        let resp: GetTxStatusResponse = match deserialize(&buf[..n]) {
            Ok(r) => r,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        };
        if !resp.error.is_empty() {
            return (StatusCode::NOT_FOUND, resp.error).into_response();
        }

        let extra = resp.block_hash.clone();
        cache.insert(txid_array, (resp.status.clone(), None, extra.clone()));
        Json(TxResponse { txid: txid_str, status: resp.status, extra_info: extra })
    }

    // GET /v1/health (aggregate delegated health)
    async fn get_health() -> impl IntoResponse {
        // Placeholder: Poll services for metrics/health
        Json("healthy")
    }

    // Background block monitor (poll for new blocks, update statuses)
    pub async fn start_block_monitor(cache: Cache) {
        loop {
            let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", BLOCK_PORT)).await {
                Ok(s) => s,
                Err(_) => {
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            let req = GetRecentBlocksRequest { token: "merchant_token".to_string() };
            let encoded = match serialize(&req) {
                Ok(e) => e,
                Err(_) => {
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            if stream.write_all(&encoded).await.is_err() || stream.flush().await.is_err() {
                sleep(Duration::from_secs(10)).await;
                continue;
            }
            let mut buf = vec![0; 1024 * 1024];
            let n = match stream.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => {
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            let resp: GetRecentBlocksResponse = match deserialize(&buf[..n]) {
                Ok(r) => r,
                Err(_) => {
                    sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            if !resp.error.is_empty() {
                sleep(Duration::from_secs(10)).await;
                continue;
            }

            for block_hex in resp.blocks {
                let block_bytes = match hex::decode(&block_hex) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let block: sv::block::Block = match sv::util::deserialize(&block_bytes) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                for tx in block.txs {
                    let txid_array = tx.txid().to_byte_array();
                    if let Some(mut entry) = cache.get_mut(&txid_array) {
                        if matches!(entry.0, TxStatus::SeenOnNetwork | TxStatus::SentToNetwork) {
                            *entry = (TxStatus::Mined, entry.1.clone(), Some(block.header.hash().to_string()));
                            // Generate proof via validation
                            let proof = ProofBundle { tx: tx.clone(), path: MerklePath::new(&block), header: block.header };
                            let mut v_stream = match TcpStream::connect(format!("127.0.0.1:{}", VALIDATION_PORT)).await {
                                Ok(s) => s,
                                Err(_) => continue,
                            };
                            let v_req = ValidateProofRequest { proof: proof.clone() };
                            let v_encoded = match serialize(&v_req) {
                                Ok(e) => e,
                                Err(_) => continue,
                            };
                            if v_stream.write_all(&v_encoded).await.is_err() || v_stream.flush().await.is_err() {
                                continue;
                            }
                            let mut v_buf = vec![0; 1024 * 1024];
                            let v_n = match v_stream.read(&mut v_buf).await {
                                Ok(n) => n,
                                Err(_) => continue,
                            };
                            let v_resp: ValidateProofResponse = match deserialize(&v_buf[..v_n]) {
                                Ok(r) => r,
                                Err(_) => continue,
                            };
                            if v_resp.success {
                                // Update cache with proof if needed
                            }
                        }
                    }
                }
            }
            sleep(Duration::from_secs(10)).await;
        }
    }

    // Background callback worker
    pub async fn start_callback_worker(cache: Cache, client: Client) {
        loop {
            for mut entry in cache.iter_mut() {
                let txid_array = *entry.key();
                let txid_str = hex::encode(txid_array);
                let (status, url_opt, extra) = entry.value_mut();
                if let Some(url) = url_opt {
                    // Assume status_changed logic
                    let callback = Callback {
                        txid: txid_str.clone(),
                        status: status.clone(),
                        block_height: None,  // Fetch if needed
                        block_hash: extra.clone(),
                        merkle_path: None,
                    };
                    let strategy = ExponentialBackoff::from_millis(100).take(5);
                    let _ = Retry::spawn(strategy, || client.post(url).json(&callback).send()).await;
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    }
}

// Spawn backgrounds in main after new
// e.g., tokio::spawn(MerchantService::start_block_monitor(cache.clone()));
// tokio::spawn(MerchantService::start_callback_worker(cache.clone(), client.clone()));
