use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use async_stream::try_stream;
use futures::Stream;
use std::pin::Pin;
use tracing::{info, warn, error};
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::Mutex;
use toml;
use governor::{Quota, RateLimiter};
use shared::ShardManager;
use sv::messages::{Tx, TxOut};
use sv::block::Block;
use sv::util::Serializable;
use sled::Db;
use std::io::Cursor;
use async_channel::Receiver;

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
struct CreateOverlayRequest {
    overlay_id: String,
    consensus_rules: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct CreateOverlayResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct JoinOverlayRequest {
    overlay_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct JoinOverlayResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct LeaveOverlayRequest {
    overlay_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct LeaveOverlayResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamOverlayDataRequest {
    overlay_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct StreamOverlayDataResponse {
    success: bool,
    data: Vec<u8>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateOverlayConsensusRequest {
    block_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateOverlayConsensusResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct AssembleOverlayBlockRequest {
    tx_hexes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct AssembleOverlayBlockResponse {
    success: bool,
    block_hex: String,
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
enum OverlayRequestType {
    CreateOverlay { request: CreateOverlayRequest, token: String },
    JoinOverlay { request: JoinOverlayRequest, token: String },
    LeaveOverlay { request: LeaveOverlayRequest, token: String },
    StreamOverlayData { request: StreamOverlayDataRequest, token: String },
    ValidateOverlayConsensus { request: ValidateOverlayConsensusRequest, token: String },
    AssembleOverlayBlock { request: AssembleOverlayBlockRequest, token: String },
    GetMetrics { request: GetMetricsRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum OverlayResponseType {
    CreateOverlay(CreateOverlayResponse),
    JoinOverlay(JoinOverlayResponse),
    LeaveOverlay(LeaveOverlayResponse),
    StreamOverlayData(StreamOverlayDataResponse),
    ValidateOverlayConsensus(ValidateOverlayConsensusResponse),
    AssembleOverlayBlock(AssembleOverlayBlockResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct OverlayService {
    auth_service_addr: String,
    alert_service_addr: String,
    db: Arc<Mutex<Db>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    alert_count: Counter,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl OverlayService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let db = Arc::new(Mutex::new(
            sled::open("overlay_db").expect("Failed to open sled database"),
        ));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("overlay_requests_total", "Total overlay requests").unwrap();
        let latency_ms = Gauge::new("overlay_latency_ms", "Average overlay request latency").unwrap();
        let alert_count = Counter::new("overlay_alert_count", "Total alerts sent").unwrap();
        let errors_total = Counter::new("overlay_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(alert_count.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        OverlayService {
            auth_service_addr: "127.0.0.1:50060".to_string(),
            alert_service_addr: "127.0.0.1:50061".to_string(),
            db,
            registry,
            requests_total,
            latency_ms,
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
            service: "overlay_service".to_string(),
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

    async fn validate_overlay_consensus(&self, block: &Block) -> Result<(), String> {
        if block.size() > 34_359_738_368 {
            return Err(format!("Block size exceeds 32GB: {}", block.size()));
        }
        for tx in &block.transactions {
            for output in &tx.outputs {
                if output.lock_script.is_op_return() && output.lock_script.size() > 4_294_967_296 {
                    return Err(format!("OP_RETURN size exceeds 4.3GB: {}", output.lock_script.size()));
                }
            }
        }
        Ok(())
    }

    async fn handle_request(&self, request: OverlayRequestType) -> Result<OverlayResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            OverlayRequestType::CreateOverlay { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "CreateOverlay").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let db = self.db.lock().await;
                let key = format!("overlay:{}", request.overlay_id);
                db.insert(key.as_bytes(), request.consensus_rules.as_bytes()).map_err(|e| {
                    warn!("Failed to create overlay: {}", e);
                    let _ = self.send_alert("create_overlay_failed", &format("Failed to create overlay: {}", e), 2);
                    format("Failed to create overlay: {}", e)
                })?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(OverlayResponseType::CreateOverlay(CreateOverlayResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            OverlayRequestType::JoinOverlay { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "JoinOverlay").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let db = self.db.lock().await;
                let key = format!("overlay:{}", request.overlay_id);
                if db.get(key.as_bytes()).map_err(|e| {
                    warn!("Failed to access overlay: {}", e);
                    let _ = self.send_alert("join_overlay_failed", &format("Failed to access overlay: {}", e), 2);
                    format("Failed to access overlay: {}", e)
                })?.is_none() {
                    warn!("Overlay not found: {}", request.overlay_id);
                    let _ = self.send_alert("join_overlay_not_found", &format("Overlay not found: {}", request.overlay_id), 2);
                    return Ok(OverlayResponseType::JoinOverlay(JoinOverlayResponse {
                        success: false,
                        error: "Overlay not found".to_string(),
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(OverlayResponseType::JoinOverlay(JoinOverlayResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            OverlayRequestType::LeaveOverlay { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "LeaveOverlay").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let db = self.db.lock().await;
                let key = format!("overlay:{}", request.overlay_id);
                if db.get(key.as_bytes()).map_err(|e| {
                    warn!("Failed to access overlay: {}", e);
                    let _ = self.send_alert("leave_overlay_failed", &format("Failed to access overlay: {}", e), 2);
                    format("Failed to access overlay: {}", e)
                })?.is_none() {
                    warn!("Overlay not found: {}", request.overlay_id);
                    let _ = self.send_alert("leave_overlay_not_found", &format("Overlay not found: {}", request.overlay_id), 2);
                    return Ok(OverlayResponseType::LeaveOverlay(LeaveOverlayResponse {
                        success: false,
                        error: "Overlay not found".to_string(),
                    }));
                }

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(OverlayResponseType::LeaveOverlay(LeaveOverlayResponse {
                    success: true,
                    error: "".to_string(),
                }))
            }
            OverlayRequestType::StreamOverlayData { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "StreamOverlayData").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let db = self.db.lock().await;
                let key = format!("overlay:{}", request.overlay_id);
                match db.get(key.as_bytes()) {
                    Ok(Some(value)) => {
                        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        Ok(OverlayResponseType::StreamOverlayData(StreamOverlayDataResponse {
                            success: true,
                            data: value.to_vec(),
                            error: "".to_string(),
                        }))
                    }
                    Ok(None) => {
                        warn!("Overlay not found: {}", request.overlay_id);
                        let _ = self.send_alert("stream_overlay_not_found", &format("Overlay not found: {}", request.overlay_id), 2);
                        Ok(OverlayResponseType::StreamOverlayData(StreamOverlayDataResponse {
                            success: false,
                            data: vec![],
                            error: "Overlay not found".to_string(),
                        }))
                    }
                    Err(e) => {
                        warn!("Failed to access overlay: {}", e);
                        let _ = self.send_alert("stream_overlay_failed", &format("Failed to access overlay: {}", e), 2);
                        Ok(OverlayResponseType::StreamOverlayData(StreamOverlayDataResponse {
                            success: false,
                            data: vec![],
                            error: format("Failed to access overlay: {}", e),
                        }))
                    }
                }
            }
            OverlayRequestType::ValidateOverlayConsensus { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "ValidateOverlayConsensus").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let block_bytes = hex::decode(&request.block_hex).map_err(|e| {
                    warn!("Invalid block hex: {}", e);
                    let _ = self.send_alert("validate_overlay_invalid_hex", &format("Invalid block hex: {}", e), 2);
                    format("Invalid block hex: {}", e)
                })?;
                let block: Block = Serializable::read(&mut Cursor::new(&block_bytes)).map_err(|e| {
                    warn!("Invalid block: {}", e);
                    let _ = self.send_alert("validate_overlay_invalid_deserialization", &format("Invalid block: {}", e), 2);
                    format("Invalid block: {}", e)
                })?;

                let result = self.validate_overlay_consensus(&block).await.map_err(|e| {
                    warn!("Overlay validation failed: {}", e);
                    let _ = self.send_alert("validate_overlay_failed", &format("Overlay validation failed: {}", e), 2);
                    e
                })?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(OverlayResponseType::ValidateOverlayConsensus(ValidateOverlayConsensusResponse {
                    success: result.is_ok(),
                    error: result.err().unwrap_or_default(),
                }))
            }
            OverlayRequestType::AssembleOverlayBlock { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "AssembleOverlayBlock").await
                    .map_err(|e| format("Authorization failed: {}", e))?;
                self.rate_limiter.until_ready().await;

                let mut block = Block::new();
                let mut total_size = 0;

                for tx_hex in request.tx_hexes {
                    let tx_bytes = hex::decode(&tx_hex).map_err(|e| {
                        warn!("Invalid transaction hex: {}", e);
                        let _ = self.send_alert("assemble_overlay_invalid_tx", &format("Invalid transaction hex: {}", e), 2);
                        format("Invalid transaction hex: {}", e)
                    })?;
                    let tx: Tx = Serializable::read(&mut Cursor::new(&tx_bytes)).map_err(|e| {
                        warn!("Invalid transaction: {}", e);
                        let _ = self.send_alert("assemble_overlay_invalid_tx_deserialization", &format("Invalid transaction: {}", e), 2);
                        format("Invalid transaction: {}", e)
                    })?;
                    total_size += tx.size();
                    if total_size > 34_359_738_368 {
                        warn!("Block size exceeds 32GB: {}", total_size);
                        let _ = self.send_alert("assemble_overlay_block_size_exceeded", &format("Block size exceeds 32GB: {}", total_size), 3);
                        return Ok(OverlayResponseType::AssembleOverlayBlock(AssembleOverlayBlockResponse {
                            success: false,
                            block_hex: "".to_string(),
                            error: "Block size exceeds 32GB".to_string(),
                        }));
                    }
                    block.add_transaction(tx);
                }

                let mut block_bytes = Vec::new();
                block.write(&mut block_bytes).unwrap();
                let block_hex = hex::encode(&block_bytes);
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(OverlayResponseType::AssembleOverlayBlock(AssembleOverlayBlockResponse {
                    success: true,
                    block_hex,
                    error: "".to_string(),
                }))
            }
            OverlayRequestType::GetMetrics { request, token } => {
                let user_id = self.authenticate(&token).await
                    .map_err(|e| format("Authentication failed: {}", e))?;
                self.authorize(&user_id, "GetMetrics").await
                    .map_err(|e| format("Authorization failed: {}", e))?;

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(OverlayResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "overlay_service".to_string(),
                    requests_total: self.requests_total.get() as u64,
                    avg_latency_ms: self.latency_ms.get(),
                    errors_total: self.errors_total.get() as u64,
                    cache_hits: 0,
                    alert_count: self.alert_count.get() as u64,
                    index_throughput: 0.0,
                }))
            }
        }
    }

    async fn stream_overlay_data(&self, requests: Receiver<StreamOverlayDataRequest>, token: String) -> impl Stream<Item = Result<StreamOverlayDataResponse, String>> {
        let service = self;
        try_stream! {
            let user_id = service.authenticate(&token).await
                .map_err(|e| format("Authentication failed: {}", e))?;
            service.authorize(&user_id, "StreamOverlayData").await
                .map_err(|e| format("Authorization failed: {}", e))?;
            service.rate_limiter.until_ready().await;

            while let Ok(req) = requests.recv().await {
                service.requests_total.inc();
                let start = std::time::Instant::now();
                let db = service.db.lock().await;
                let key = format!("overlay:{}", req.overlay_id);
                match db.get(key.as_bytes()) {
                    Ok(Some(value)) => {
                        service.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                        yield StreamOverlayDataResponse {
                            success: true,
                            data: value.to_vec(),
                            error: "".to_string(),
                        };
                    }
                    Ok(None) => {
                        warn!("Overlay not found: {}", req.overlay_id);
                        let _ = service.send_alert("stream_overlay_not_found", &format("Overlay not found: {}", req.overlay_id), 2);
                        yield StreamOverlayDataResponse {
                            success: false,
                            data: vec![],
                            error: "Overlay not found".to_string(),
                        };
                    }
                    Err(e) => {
                        warn!("Failed to access overlay: {}", e);
                        let _ = service.send_alert("stream_overlay_failed", &format("Failed to access overlay: {}", e), 2);
                        yield StreamOverlayDataResponse {
                            success: false,
                            data: vec![],
                            error: format("Failed to access overlay: {}", e),
                        };
                    }
                }
            }
        }
    }

    async fn run(&self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;
        info!("Overlay service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: OverlayRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = OverlayResponseType::CreateOverlay(CreateOverlayResponse {
                                            success: false,
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

    let addr = "127.0.0.1:50056";
    let overlay_service = OverlayService::new().await;
    overlay_service.run(addr).await?;
    Ok(())
}
