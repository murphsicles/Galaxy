use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use async_stream::try_stream;
use futures::Stream;
use std::pin::Pin;
use tracing::{info, warn, error};
use lru::LruCache;
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::Mutex;
use toml;
use governor::{Quota, RateLimiter};
use shared::ShardManager;
use sv::transaction::Transaction;
use sv::util::{deserialize as sv_deserialize, hash::Sha256d, serialize};

// Temporary: Keep block_client and storage_client until updated
use block::block_client::BlockClient;
use block::GetBlockHeadersRequest;
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use tonic::transport::Channel;

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
    auth_service_addr: String,
    alert_service_addr: String,
    block_client: BlockClient<Channel>,
    storage_client: StorageClient<Channel>,
    proof_cache: Arc<Mutex<LruCache<String, (String, Vec<String>)>>>,
    registry: Arc<Registry>,
    proof_requests: Counter,
    latency_ms: Gauge,
    cache_hits: Counter,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl ValidationService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
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
            block_service_addr: "127.0.0.1:50054".to_string(),
            storage_service_addr: "127.0.0.1:50053".to_string(),
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
            block_client,
            storage_client,
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
            .map_err(|e| format("Failed to connect to auth_service: {}", e))?;
        let request = AuthRequest { token: token.to_string() };
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
        let response: AuthResponse = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
        if response.success {
            Ok(response.user_id)
        } else {
            Err(response.error)
        }
    }

    async fn authorize(&self, user_id: &str, method: &str) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.auth_service_addr).await
            .map_err(|e| format("Failed to connect to auth_service: {}", e))?;
        let request = AuthorizeRequest {
            user_id: user_id.to_string(),
            service: "validation_service".to_string(),
            method: method.to_string(),
        };
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
        let response: AuthorizeResponse = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
        if response.allowed {
            Ok(())
        } else {
            Err(response.error)
        }
    }

    async fn send_alert(&self, event_type: &str, message: &str, severity: u32) -> Result<(), String> {
        let mut stream = TcpStream::connect(&self.alert_service_addr).await
            .map_err(|e| format("Failed to connect to alert_service: {}", e))?;
        let request = AlertRequest {
            event_type: event_type.to_string(),
            message: message.to_string(),
            severity,
        };
        let encoded = serialize(&request).map_err(|e| format("Serialization error: {}", e))?;
        stream.write_all(&encoded).await.map_err(|e| format("Write error: {}", e))?;
        stream.flush().await.map_err(|e| format("Flush error: {}", e))?;

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| format("Read error: {}", e))?;
        let response: AlertResponse = deserialize(&buffer[..n])
            .map_err(|e| format("Deserialization error: {}", e))?;
        
        if response.success {
            self.alert_count.inc();
            Ok(())
        } else {
            warn!("Alert sending failed: {}", response.error);
            Err(response.error)
        }
    }

    async fn generate_spv_proof(&self, txid: &str) -> Result<(String, Vec<String>), String> {
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
            let _ = self.send_alert("spv_invalid_txid", &format("Invalid txid: {}", e), 2);
            format("Invalid txid: {}", e)
        })?;
        let tx: Transaction = sv_deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self.send_alert("spv_invalid_transaction", &format("Invalid transaction: {}", e), 2);
            format("Invalid transaction: {}", e)
        })?;

        let block_request = GetBlockHeadersRequest {
            block_hash: tx.txid().to_string(),
        };
        let block_response = self
            .block_client
            .get_block_headers(block_request)
            .await
            .map_err(|e| {
                warn!("Failed to fetch block headers: {}", e);
                let _ = self.send_alert("spv_block_fetch_failed", &format("Failed to fetch block headers: {}", e), 2);
                format("Failed to fetch block headers: {}", e)
            })?;
        let block_response = block_response.into_inner();

        if block_response.headers.is_empty() {
            warn!("No block headers found for txid: {}", txid);
            let _ = self.send_alert("spv_no_headers", &format("No block headers found for txid: {}", txid), 3);
            return Err("No block headers found".to_string());
        }

        let merkle_path = self.calculate_merkle_path(&tx).await.map_err(|e| {
            warn!("Failed to calculate merkle path: {}", e);
            let _ = self.send_alert("spv_merkle_path_failed", &format("Failed to calculate merkle path: {}", e), 2);
            format("Failed to calculate merkle path: {}", e)
        })?;

        let merkle_path_hex = hex::encode(serialize(&merkle_path));
        proof_cache.put(txid.to_string(), (merkle_path_hex.clone(), block_response.headers.clone()));
        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);

        Ok((merkle_path_hex, block_response.headers))
    }

    async fn calculate_merkle_path(&self, tx: &Transaction) -> Result<Vec<Sha256d>, String> {
        info!("Calculating merkle path for txid: {}", tx.txid());
        Ok(vec![]) // Placeholder
    }

    async fn verify_spv_proof(&self, txid: &str, merkle_path: &str, block_headers: &[String]) -> Result<bool, String> {
        self.proof_requests.inc();
        let start = std::time::Instant::now();

        let tx_bytes = hex::decode(txid).map_err(|e| {
            warn!("Invalid txid: {}", e);
            let _ = self.send_alert("spv_verify_invalid_txid", &format("Invalid txid: {}", e), 2);
            format("Invalid txid: {}", e)
        })?;
        let _tx: Transaction = sv_deserialize(&tx_bytes).map_err(|e| {
            warn!("Invalid transaction: {}", e);
            let _ = self.send_alert("spv_verify_invalid_transaction", &format("Invalid transaction: {}", e), 2);
            format("Invalid transaction: {}", e)
        })?;

        let merkle_path_bytes = hex::decode(merkle_path).map_err(|e| {
            warn!("Invalid merkle path: {}", e);
            let _ = self.send_alert("spv_verify_invalid_merkle_path", &format("Invalid merkle path: {}", e), 2);
            format("Invalid merkle path: {}", e)
        })?;
        let _merkle_path: Vec<Sha256d> = deserialize(&merkle_path_bytes).map_err(|e| {
            warn!("Invalid merkle path deserialization: {}", e);
            let _ = self.send_alert("spv_verify_merkle_path_deserialization", &format("Invalid merkle path: {}", e), 2);
            format("Invalid merkle path deserialization: {}", e)
        })?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(true) // Placeholder
    }

    async fn handle_request(&self, request: ValidationRequestType) -> Result<ValidationResponseType, String> {
        self.proof_requests.inc();
        let start = std::time::Instant::now();

        match request {
            ValidationRequestType::GenerateSPVProof { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GenerateSPVProof").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let (merkle_path, block_headers) = self.generate_spv_proof(&request.txid).await?;

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
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "VerifySPVProof").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
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
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "BatchGenerateSPVProof").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut results = vec![];
                let mut proof_cache = self.proof_cache.lock().await;

                for txid in request.txids {
                    let result = self.generate_spv_proof(&txid).await;
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
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "StreamSPVProofs").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let result = self.generate_spv_proof(&request.txid).await;
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
                        let _ = self.send_alert("spv_proof_generation_failed", &format("Failed to generate SPV proof: {}", e), 2);
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
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format("Authorization failed: {}", e))?;

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

    async fn stream_spv_proofs(&self, requests: Receiver<StreamSPVProofsRequest>, token: String) -> impl Stream<Item = Result<StreamSPVProofsResponse, String>> {
        let service = self;
        try_stream! {
            let user_id = service.authenticate(&token).await
                .map_err(|e| format("Authentication failed: {}", e))?;
            service.authorize(&user_id, "StreamSPVProofs").await
                .map_err(|e| format("Authorization failed: {}", e))?;
            service.rate_limiter.until_ready().await;

            while let Ok(req) = requests.recv().await {
                service.proof_requests.inc();
                let start = std::time::Instant::now();

                let result = service.generate_spv_proof(&req.txid).await;
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
                        let _ = service.send_alert("spv_proof_generation_failed", &format("Failed to generate SPV proof: {}", e), 2);
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "127.0.0.1:50057";
    let validation_service = ValidationService::new().await;
    validation_service.run(addr).await?;
    Ok(())
}
