use std::env;
use std::io::Cursor;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;

use async_stream::try_stream;
use bincode::{deserialize, serialize};
use dotenv::dotenv;
use futures::Stream;
use governor::{Quota, RateLimiter};
use hex;
use lru::LruCache;
use prometheus::{Counter, Gauge, Registry};
use serde::{Deserialize, Serialize};
use shared::ShardManager;
use sv::block::Header;
use sv::messages::Tx;
use sv::util::{Hash256, Serializable};
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
struct GenerateSPVProofRequest {
    txid: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GenerateSPVProofResponse {
    success: bool,
    merkle_path: String,
    block_headers: Vec<String>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct VerifySPVProofRequest {
    txid: String,
    merkle_path: String,
    block_headers: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct VerifySPVProofResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchGenerateSPVProofRequest {
    txids: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct BatchGenerateSPVProofResponse {
    results: Vec<GenerateSPVProofResponse>,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamSPVProofsRequest {
    txid: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamSPVProofsResponse {
    success: bool,
    merkle_path: String,
    block_headers: Vec<String>,
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
struct GetBlockHeadersRequest {
    block_hash: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetBlockHeadersResponse {
    headers: Vec<String>,
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
struct GetTxStatusRequest {
    txid: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetTxStatusResponse {
    status: String,
    block_hash: Option<String>,
    block_height: Option<u64>,
    merkle_path: Option<String>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum BlockRequestType {
    GetBlockHeaders { request: GetBlockHeadersRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum BlockResponseType {
    GetBlockHeaders(GetBlockHeadersResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageRequestType {
    QueryUtxo { request: QueryUtxoRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum StorageResponseType {
    QueryUtxo(QueryUtxoResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexRequestType {
    GetTxStatus { request: GetTxStatusRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum IndexResponseType {
    GetTxStatus(GetTxStatusResponse),
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationRequestType {
    GenerateSPVProof { request: GenerateSPVProofRequest, token: String },
    VerifySPVProof { request: VerifySPVProofRequest, token: String },
    BatchGenerateSPVProof { request: BatchGenerateSPVProofRequest, token: String },
    StreamSPVProofs { request: StreamSPVProofsRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationResponseType {
    GenerateSPVProof(GenerateSPVProofResponse),
    VerifySPVProof(VerifySPVProofResponse),
    BatchGenerateSPVProof(BatchGenerateSPVProofResponse),
    StreamSPVProofs(StreamSPVProofsResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct ValidationService {
    block_service_addr: String,
    storage_service_addr: String,
    index_service_addr: String,
    auth_service_addr: String,
    alert_service_addr: String,
    proof_cache: Arc<Mutex<LruCache<String, (String, Vec<String>)>>>,
    registry: Arc<Registry>,
    proof_requests: Counter,
    latency_ms: Gauge,
    cache_hits: Counter,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<NotKeyed, governor::state::InMemoryState, DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl ValidationService {
    async fn new() -> Self {
        dotenv().ok();

        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let proof_cache = Arc::new(Mutex::new(LruCache::new(NonZeroU32::new(1000).unwrap())));
        let registry = Arc::new(Registry::new());
        let proof_requests = Counter::new("validation_requests_total", "Total SPV proof requests").unwrap();
        let latency_ms = Gauge::new("validation_latency_ms", "Average proof generation latency").unwrap();
        let cache_hits = Counter::new("validation_cache_hits", "Total cache hits").unwrap();
        let alert_count = Counter::new("validation_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("validation_errors_total", "Total errors").unwrap();
        registry.register(Box::new(proof_requests.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(cache_hits.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        ValidationService {
            block_service_addr: env::var("BLOCK_ADDR").unwrap_or("127.0.0.1:50054".to_string()),
            storage_service_addr: env::var("STORAGE_ADDR").unwrap_or("127.0.0.1:50053".to_string()),
            index_service_addr: env::var("INDEX_ADDR").unwrap_or("127.0.0.1:50059".to_string()),
            auth_service_addr: env::var("AUTH_ADDR").unwrap_or("127.0.0.1:50060".to_string()),
            alert_service_addr: env::var("ALERT_ADDR").unwrap_or("127.0.0.1:50061".to_string()),
            proof_cache,
            registry,
            proof_requests,
            latency_ms,
            cache_hits,
            alert_count,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
        }
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
            service: "validation_service".to_string(),
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
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
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

    async fn get_tx_status(&self, txid: &str, token: &str) -> Result<GetTxStatusResponse, String> {
        let mut stream = TcpStream::connect(&self.index_service_addr).await
            .map_err(|e| format!("Failed to connect to index_service: {}", e))?;
        let request = IndexRequestType::GetTxStatus { request: GetTxStatusRequest { txid: txid.to_string() }, token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
        let response: IndexResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;

        if let IndexResponseType::GetTxStatus(resp) = response {
            Ok(resp)
        } else {
            Err("Unexpected response type".to_string())
        }
    }

    async fn generate_spv_proof(&self, txid: &str, token: &str) -> Result<(String, Vec<String>), String> {
        self.proof_requests.inc();
        let start = std::time::Instant::now();

        let mut proof_cache = self.proof_cache.lock().await;
        if let Some(cached) = proof_cache.get(txid) {
            self.cache_hits.inc();
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(cached.clone());
        }

        let tx_bytes = hex::decode(txid).map_err(|e| {
            warn!("Invalid txid: {}", e);
            let _ = self.send_alert("spv_invalid_txid", &format!("Invalid txid: {}", e), 2);
            format!("Invalid txid: {}", e)
        })?;
        let tx: Tx = Serializable::read(&mut Cursor::new(&tx_bytes)).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self.send_alert("spv_invalid_transaction", &format!("Invalid transaction: {}", e), 2);
            format!("Invalid transaction: {}", e)
        })?;

        let status = self.get_tx_status(txid, token).await?;
        if status.error.is_empty() && status.block_hash.is_none() {
            warn!("No block hash found for txid: {}", txid);
            let _ = self.send_alert("spv_no_block_hash", &format!("No block hash found for txid: {}", txid), 3);
            return Err("No block hash found".to_string());
        }
        let block_hash = status.block_hash.unwrap_or_default();

        let mut stream = TcpStream::connect(&self.block_service_addr).await
            .map_err(|e| format!("Failed to connect to block_service: {}", e))?;
        let block_request = BlockRequestType::GetBlockHeaders { request: GetBlockHeadersRequest { block_hash }, token: token.to_string() };
        let encoded = serialize(&block_request).map_err(|e| format!("Serialization error: {}", e))?;
        stream.write_all(&encoded).await map_err(|e| format!("Write error: {}", e))?;
        stream.flush().await map_err(|e| format!("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await map_err(|e| format!("Read error: {}", e))?;
        let block_response: BlockResponseType = deserialize(&buffer[..n])
            .map_err(|e| format!("Deserialization error: {}", e))?;
        
        let headers = match block_response {
            BlockResponseType::GetBlockHeaders(response) => response.headers,
            _ => {
                warn!("Unexpected response from block_service");
                let _ = self.send_alert("spv_block_fetch_failed", "Unexpected response from block_service", 2);
                return Err("Unexpected response from block_service".to_string());
            }
        };

        if headers.is_empty() {
            warn!("No block headers found for txid: {}", txid);
            let _ = self.send_alert("spv_no_headers", &format!("No block headers found for txid: {}", txid), 3);
            return Err("No block headers found".to_string());
        }

        // Fetch actual block for tx_hashes (add GetBlockByHash to block_service if needed; placeholder with dummy)
        let dummy_tx_hashes = vec![tx.hash(), Hash256::default()];
        let path = self.calculate_merkle_path(&tx.hash().0, &dummy_tx_hashes);

        let merkle_path_hex = hex::encode(path.iter().flatten().cloned().collect::<Vec<u8>>());

        proof_cache.put(txid.to_string(), (merkle_path_hex.clone(), headers.clone()));
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);

        Ok((merkle_path_hex, headers))
    }

    fn calculate_merkle_path(&self, tx_hash: &[u8; 32], tx_hashes: &Vec<Hash256>) -> Vec<[u8; 32]> {
        let mut path = Vec::new();
        if let Some(mut idx) = tx_hashes.iter().position(|h| h.0 == *tx_hash) {
            let mut level = tx_hashes.clone();
            while level.len() > 1 {
                let sibling_idx = idx ^ 1;
                let sibling = if sibling_idx < level.len() {
                    level[sibling_idx].0
                } else {
                    level[idx].0
                };
                path.push(sibling);
                let mut parent_level = Vec::new();
                for i in (0..level.len()).step_by(2) {
                    let left = level[i].0;
                    let right = if i + 1 < level.len() { level[i + 1].0 } else { left };
                    let parent = Hash256::hash(&[left, right].concat()).0;
                    parent_level.push(Hash256(parent));
                }
                level = parent_level;
                idx /= 2;
            }
        }
        path
    }

    async fn verify_spv_proof(&self, txid: &str, merkle_path: &str, block_headers: &[String]) -> Result<bool, String> {
        self.proof_requests.inc();
        let start = std::time::Instant::now();

        let tx_hash_bytes = hex::decode(txid).map_err(|e| {
            warn!("Invalid txid: {}", e);
            let _ = self.send_alert("spv_verify_invalid_txid", &format!("Invalid txid: {}", e), 2);
            format!("Invalid txid: {}", e)
        })?;
        let mut current_hash = Hash256::hash(&tx_hash_bytes).0;

        let path_bytes = hex::decode(merkle_path).map_err(|e| {
            warn!("Invalid merkle path: {}", e);
            let _ = self.send_alert("spv_verify_invalid_merkle_path", &format!("Invalid merkle path: {}", e), 2);
            format!("Invalid merkle path: {}", e)
        })?;
        let path: Vec<[u8; 32]> = path_bytes.chunks(32).map(|chunk| chunk.try_into().unwrap()).collect();

        for sibling in path {
            current_hash = if current_hash < sibling {
                Hash256::hash(&[current_hash, sibling].concat()).0
            } else {
                Hash256::hash(&[sibling, current_hash].concat()).0
            };
        }

        // Assume first header is the block header
        let header_bytes = hex::decode(&block_headers[0]).map_err(|e| {
            warn!("Invalid header: {}", e);
            let _ = self.send_alert("spv_verify_invalid_header", &format!("Invalid header: {}", e), 2);
            format!("Invalid header: {}", e)
        })?;
        let header: Header = Serializable::read(&mut Cursor::new(&header_bytes)).map_err(|e| {
            warn!("Invalid header deserialization: {}", e);
            let _ = self.send_alert("spv_verify_header_deserialization", &format!("Invalid header: {}", e), 2);
            format!("Invalid header deserialization: {}", e)
        })?;

        let is_valid = current_hash == header.merkle_root.0;

        if !is_valid {
            let _ = self.send_alert("spv_proof_invalid", "SPV proof verification failed", 3);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(is_valid)
    }

    async fn handle_request(&self, request: ValidationRequestType) -> Result<ValidationResponseType, String> {
        self.proof_requests.inc();
        let start = std::time::Instant::now();

        match request {
            ValidationRequestType::GenerateSPVProof { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GenerateSPVProof").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let (merkle_path, block_headers) = self.generate_spv_proof(&request.txid, &token).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ValidationResponseType::GenerateSPVProof(GenerateSPVProofResponse {
                    success: true,
                    merkle_path,
                    block_headers,
                    error: "".to_string(),
                }))
            }
            ValidationRequestType::VerifySPVProof { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "VerifySPVProof").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let is_valid = self.verify_spv_proof(&request.txid, &request.merkle_path, &request.block_headers).await?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ValidationResponseType::VerifySPVProof(VerifySPVProofResponse {
                    success: is_valid,
                    error: if is_valid { "".to_string() } else { "Proof verification failed".to_string() },
                }))
            }
            ValidationRequestType::BatchGenerateSPVProof { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "BatchGenerateSPVProof").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut results = vec![];
                let mut proof_cache = self.proof_cache.lock().await;

                for txid in request.txids {
                    let result = self.generate_spv_proof(&txid, &token).await;
                    match result {
                        Ok((merkle_path_hex, block_headers)) => {
                            proof_cache.put(txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
                            results.push(GenerateSPVProofResponse {
                                success: true,
                                merkle_path: merkle_path_hex,
                                block_headers,
                                error: "".to_string(),
                            });
                        }
                        Err(e) => {
                            results.push(GenerateSPVProofResponse {
                                success: false,
                                merkle_path: "".to_string(),
                                block_headers: vec![],
                                error: e,
                            });
                        }
                    }
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ValidationResponseType::BatchGenerateSPVProof(BatchGenerateSPVProofResponse { results }))
            }
            ValidationRequestType::StreamSPVProofs { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "StreamSPVProofs").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let result = self.generate_spv_proof(&request.txid, &token).await;
                match result {
                    Ok((merkle_path_hex, block_headers)) => {
                        let mut proof_cache = self.proof_cache.lock().await;
                        proof_cache.put(request.txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
                        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        Ok(ValidationResponseType::StreamSPVProofs(StreamSPVProofsResponse {
                            success: true,
                            merkle_path: merkle_path_hex,
                            block_headers,
                            error: "".to_string(),
                        }))
                    }
                    Err(e) => {
                        warn!("Failed to generate SPV proof: {}", e);
                        let _ = self.send_alert("spv_proof_generation_failed", &format!("Failed to generate SPV proof: {}", e), 2);
                        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        Ok(ValidationResponseType::StreamSPVProofs(StreamSPVProofsResponse {
                            success: false,
                            merkle_path: "".to_string(),
                            block_headers: vec![],
                            error: e,
                        }))
                    }
                }
            }
            ValidationRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format!("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format!("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ValidationResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "validation_service".to_string(),
                    requests_total: self.proof_requests.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: self.cache_hits.get() as u64,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                }))
            }
        }
    }

    async fn stream_spv_proofs(&self, requests: Receiver<StreamSPVProofsRequest>, token: String) -> impl Stream<Item = Result<StreamSPVProofsResponse, String>> {
        let service = self;
        try_stream! {
            let user_id = service.authenticate(&token).await
                .map_err(|e| format!("Authentication failed: {}", e))?;
            service.authorize(&user_id, "StreamSPVProofs").await
                .map_err(|e| format!("Authorization failed: {}", e))?;
            service.rate_limiter.until_ready().await;

            while let Ok(req) = requests.recv().await {
                service.proof_requests.inc();
                let start = std::time::Instant::now();

                let result = service.generate_spv_proof(&req.txid, &token).await;
                match result {
                    Ok((merkle_path_hex, block_headers)) => {
                        let mut proof_cache = service.proof_cache.lock().await;
                        proof_cache.put(req.txid.clone(), (merkle_path_hex.clone(), block_headers.clone()));
                        service.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        yield StreamSPVProofsResponse {
                            success: true,
                            merkle_path: merkle_path_hex,
                            block_headers,
                            error: "".to_string(),
                        };
                    }
                    Err(e) => {
                        warn!("Failed to generate SPV proof: {}", e);
                        let _ = service.send_alert("spv_proof_generation_failed", &format!("Failed to generate SPV proof: {}", e), 2);
                        service.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        yield StreamSPVProofsResponse {
                            success: false,
                            merkle_path: "".to_string(),
                            block_headers: vec![],
                            error: e,
                        };
                    }
                }
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Validation service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: ValidationRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = ValidationResponseType::GenerateSPVProof(GenerateSPVProofResponse {
                                            success: false,
                                            merkle_path: "".to_string(),
                                            block_headers: vec![],
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

    let addr = env::var("VALIDATION_ADDR").unwrap_or("127.0.0.1:50057".to_string());
    let validation_service = ValidationService::new().await;
    validation_service.run(&addr).await?;
    Ok(())
}
