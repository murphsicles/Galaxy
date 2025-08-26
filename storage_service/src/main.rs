use bincode::{deserialize, serialize};
use dotenv::dotenv;
use governor::{Quota, RateLimiter};
use hex;
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sled::Db;
use std::env;
use std::num::NonZeroU32;
use std::sync::Arc;
use sv::block::Block;
use sv::util::Hash256;
use tigerbeetle_unofficial as tigerbeetle;
use tigerbeetle::{Account, Transfer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use toml;
use tracing::{error, info, warn};

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

#[derive(Serialize, Deserialize, Debug)]
struct AddUtxoRequest {
    txid: String,
    vout: u32,
    script_pubkey: String,
    amount: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct AddUtxoResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct RemoveUtxoRequest {
    txid: String,
    vout: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct RemoveUtxoResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchAddUtxoRequest {
    utxos: Vec<AddUtxoRequest>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchAddUtxoResponse {
    results: Vec<AddUtxoResponse>,
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
struct GetBlocksByTimestampRequest {
    before_timestamp: i64,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByTimestampResponse {
    blocks: Vec<Block>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByHeightRequest {
    max_height: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlocksByHeightResponse {
    blocks: Vec<Block>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageRequestType {
    QueryUtxo { request: QueryUtxoRequest, token: String },
    AddUtxo { request: AddUtxoRequest, token: String },
    RemoveUtxo { request: RemoveUtxoRequest, token: String },
    BatchAddUtxo { request: BatchAddUtxoRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
    GetBlocksByTimestamp { request: GetBlocksByTimestampRequest, token: String },
    GetBlocksByHeight { request: GetBlocksByHeightRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    QueryUtxo(QueryUtxoResponse),
    AddUtxo(AddUtxoResponse),
    RemoveUtxo(RemoveUtxoResponse),
    BatchAddUtxo(BatchAddUtxoResponse),
    GetMetrics(GetMetricsResponse),
    GetBlocksByTimestamp(GetBlocksByTimestampResponse),
    GetBlocksByHeight(GetBlocksByHeightResponse),
}

#[derive(Debug)]
struct StorageService {
    tb_client: Arc<Mutex<tigerbeetle::Client>>,
    script_db: Arc<Db>,
    auth_service_addr: String,
    alert_service_addr: String,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<gov::state::NotKeyed, gov::state::InMemoryState, gov::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
    transfer_id: Arc<Mutex<u128>>,
}

impl StorageService {
    async fn new() -> Self {
        dotenv().ok();

        let tb_addr = env::var("TB_ADDR").unwrap_or("127.0.0.1:3000".to_string());
        let cluster_id = env::var("TB_CLUSTER_ID").unwrap_or("1".to_string()).parse::<u32>().unwrap();
        let tb_client = tigerbeetle::Client::new(&[tb_addr], cluster_id).await.expect("Failed to connect to Tiger Beetle");

        let sink = Account {
            id: 0,
            debit_posted: 0,
            debit_accepted: 0,
            credit_posted: 0,
            user_data_128: 0,
            user_data_64: 0,
            user_data_32: 0,
            reserved: [0; 48],
            ledger: 700,
            code: 2,
            flags: 0,
            timestamp: 0,
        };
        tb_client.create_accounts(&[sink]).await.ok(); // Ignore if exists

        let script_db = sled::open("utxo_scripts.db").expect("Failed to open sled DB");

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("storage_requests_total", "Total storage requests").unwrap();
        let latency_ms = Gauge::new("storage_latency_ms", "Average storage request latency").unwrap();
        let alert_count = Counter::new("storage_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("storage_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        StorageService {
            tb_client: Arc::new(Mutex::new(tb_client)),
            script_db: Arc::new(script_db),
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            registry,
            requests_total,
            latency_ms,
            alert_count,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
            transfer_id: Arc::new(Mutex::new(0)),
        }
    }

    fn hash_to_u128(key: &str) -> u128 {
        let hash = Hash256::hash(key.as_bytes());
        u128::from_le_bytes(hash.0[0..16].try_into().unwrap())
    }

    async fn authenticate(&self, token: &str) -> Result<String, String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await
            .map_err(|e| format!("Failed to connect to auth_service: {}", e))?;
        let request = AuthRequest { token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: AuthResponse = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        if response.success {
            Ok(response.user_id)
        } else {
            Err(response.error)
        }
    }

    async fn authorize(&self, user_id: &str, method: &str) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await
            .map_err(|e| format!("Failed to connect to auth_service: {}", e))?;
        let request = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "storage_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: AuthorizeResponse = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        if response.allowed {
            Ok(())
        } else {
            Err(response.error)
        }
    }

    async fn send_alert(&self, event_type: &str, message: &str, severity: u32) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.alert_service_addr).await
            .map_err(|e| format!("Failed to connect to alert_service: {}", e))?;
        let request = AlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format!("Read error: {}", e))?;
        let response: AlertResponse = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        if response.success {
            self.alert_count.inc();
            Ok(())
        } else {
            warn!("Alert sending failed: {}", response.error);
            Err(response.error)
        }
    }

    async fn handle_request(&self, request: StorageRequestType) -> Result<StorageResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            StorageRequestType::QueryUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "QueryUtxo").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let key = format!("{}:{}", request.txid, request.vout);
                let id = Self::hash_to_u128(&key);
                let mut client = self.tb_client.lock().await;
                let accounts = client.lookup_accounts(&[id]).await.map_err(|e| format!("Lookup error: {:?}", e))?;
                let exists = !accounts.is_empty() && accounts[0].credit_posted > accounts[0].debit_posted;
                let amount = if exists { accounts[0].credit_posted - accounts[0].debit_posted } else { 0 };
                let script_pubkey = if exists {
                    String::from_utf8(self.script_db.get(key.as_bytes()).map_err(|e| format!("Sled error: {}", e))?.unwrap_or_default()).map_err(|e| format!("UTF8 error: {}", e))?
                } else {
                    "".to_string()
                };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::QueryUtxo(QueryUtxoResponse {
                    exists,
                    script_pubkey,
                    amount,
                    error: if exists { "".to_string() } else { "UTXO not found".to_string() },
                }))
            }
            StorageRequestType::AddUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "AddUtxo").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let key = format!("{}:{}", request.txid, request.vout);
                let id = Self::hash_to_u128(&key);
                let account = Account {
                    id,
                    debit_posted: 0,
                    debit_accepted: 0,
                    credit_posted: request.amount,
                    user_data_128: 0,
                    user_data_64: 0,
                    user_data_32: 0,
                    reserved: [0; 48],
                    ledger: 700,
                    code: 1,
                    flags: 0,
                    timestamp: 0,
                };
                let mut client = self.tb_client.lock().await;
                let errs = client.create_accounts(&[account]).await.map_err(|e| format!("Create error: {:?}", e))?;
                if !errs.is_empty() {
                    return Err(format!("Create account errors: {:?}", errs));
                }
                self.script_db.insert(key.as_bytes(), request.script_pubkey.as_bytes()).map_err(|e| format!("Sled error: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::AddUtxo(AddUtxoResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            StorageRequestType::RemoveUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "RemoveUtxo").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let key = format!("{}:{}", request.txid, request.vout);
                let id = Self::hash_to_u128(&key);
                let mut client = self.tb_client.lock().await;
                let accounts = client.lookup_accounts(&[id]).await.map_err(|e| format!("Lookup error: {:?}", e))?;
                if accounts.is_empty() {
                    warn!("UTXO not found for txid: {}", request.txid);
                    let _ = self.send_alert("remove_utxo_not_found", &format!("UTXO not found for txid: {}", request.txid), 2);
                    return Ok(StorageResponseType::RemoveUtxo(RemoveUtxoResponse {
                        success: false,
                        error: "UTXO not found".to_string(),
                    }));
                }
                let amount = accounts[0].credit_posted - accounts[0].debit_posted;
                let mut transfer_id = self.transfer_id.lock().await;
                *transfer_id += 1;
                let transfer = Transfer {
                    id: *transfer_id,
                    debit_account_id: id,
                    credit_account_id: 0,
                    amount,
                    pending_id: 0,
                    user_data_128: 0,
                    user_data_64: 0,
                    user_data_32: 0,
                    timeout: 0,
                    ledger: 700,
                    code: 2,
                    flags: 0,
                    timestamp: 0,
                };
                let errs = client.create_transfers(&[transfer]).await.map_err(|e| format!("Transfer error: {:?}", e))?;
                if !errs.is_empty() {
                    return Err(format!("Transfer errors: {:?}", errs));
                }
                self.script_db.remove(key.as_bytes()).map_err(|e| format!("Sled error: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::RemoveUtxo(RemoveUtxoResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            StorageRequestType::BatchAddUtxo { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "BatchAddUtxo").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut accounts = vec![];
                let mut results = vec![];
                for utxo in request.utxos {
                    let key = format!("{}:{}", utxo.txid, utxo.vout);
                    let id = Self::hash_to_u128(&key);
                    let account = Account {
                        id,
                        debit_posted: 0,
                        debit_accepted: 0,
                        credit_posted: utxo.amount,
                        user_data_128: 0,
                        user_data_64: 0,
                        user_data_32: 0,
                        reserved: [0; 48],
                        ledger: 700,
                        code: 1,
                        flags: 0,
                        timestamp: 0,
                    };
                    accounts.push(account);
                    self.script_db.insert(key.as_bytes(), utxo.script_pubkey.as_bytes()).map_err(|e| format!("Sled error: {}", e))?;
                    results.push(AddUtxoResponse {
                        success: true,
                        error: "".to_string(),
                    });
                }
                let mut client = self.tb_client.lock().await;
                let errs = client.create_accounts(&accounts).await.map_err(|e| format!("Create error: {:?}", e))?;
                if !errs.is_empty() {
                    return Err(format!("Batch create errors: {:?}", errs));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::BatchAddUtxo(BatchAddUtxoResponse { results }))
            }
            StorageRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "storage_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                }))
            }
            StorageRequestType::GetBlocksByTimestamp { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetBlocksByTimestamp").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                // Placeholder (full implementation requires block storage API)
                let blocks = vec![];

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(StorageResponseType::GetBlocksByTimestamp(GetBlocksByTimestampResponse { blocks, error: "".to_string() }))
            }
            StorageRequestType::GetBlocksByHeight { request, token } => {
            
