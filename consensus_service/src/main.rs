use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use sv::transaction::Transaction;
use sv::util::{deserialize as sv_deserialize};
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use toml;
use governor::{Quota, RateLimiter};
use shared::ShardManager;

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTransactionConsensusRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTransactionConsensusResponse {
    success: bool,
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
enum ConsensusRequestType {
    ValidateTransaction { request: ValidateTransactionConsensusRequest },
    GetMetrics { request: GetMetricsRequest },
}

#[derive(Serialize, Deserialize, Debug)]
enum ConsensusResponseType {
    ValidateTransaction(ValidateTransactionConsensusResponse),
    GetMetrics(GetMetricsResponse),
}

#[derive(Debug)]
struct ConsensusService {
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
    errors_total: Counter,
    rate_limiter: Arc<RateLimiter<String, governor::state::direct::NotKeyed, governor::clock::DefaultClock>>,
    shard_manager: Arc<ShardManager>,
}

impl ConsensusService {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("consensus_requests_total", "Total consensus requests").unwrap();
        let latency_ms = Gauge::new("consensus_latency_ms", "Average consensus request latency").unwrap();
        let errors_total = Counter::new("consensus_errors_total", "Total errors").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();
        registry.register(Box::new(errors_total.clone())).unwrap();
        let rate_limiter = Arc::new(RateLimiter::direct(Quota::per_second(
            NonZeroU32::new(1000).unwrap(),
        )));

        ConsensusService {
            registry,
            requests_total,
            latency_ms,
            errors_total,
            rate_limiter,
            shard_manager: Arc::new(ShardManager::new()),
        }
    }

    async fn handle_request(&self, request: ConsensusRequestType) -> Result<ConsensusResponseType, String> {
        self.requests_total.inc();
        let start = std::time::Instant::now();

        match request {
            ConsensusRequestType::ValidateTransaction { request } => {
                let tx_bytes = hex::decode(&request.tx_hex).map_err(|e| {
                    warn!("Invalid transaction hex: {}", e);
                    format("Invalid transaction hex: {}", e)
                })?;
                let tx: Transaction = sv_deserialize(&tx_bytes).map_err(|e| {
                    warn!("Invalid transaction: {}", e);
                    format("Invalid transaction: {}", e)
                })?;

                // Placeholder: Validate transaction against consensus rules
                let success = true; // Replace with actual consensus validation logic
                let error = if success { "".to_string() } else { "Validation failed".to_string() };

                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
                    success,
                    error,
                }))
            }
            ConsensusRequestType::GetMetrics { request } => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                Ok(ConsensusResponseType::GetMetrics(GetMetricsResponse {
                    service_name: "consensus_service".to_string(),
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
        info!("Consensus service running on {}", addr);

        loop {
            match listener.accept().await {
                Ok((mut stream, addr)) => {
                    let service = self;
                    tokio::spawn(async move {
                        let mut buffer = vec![0u8; 1024 * 1024];
                        match stream.read(&mut buffer).await {
                            Ok(n) => {
                                let request: ConsensusRequestType = match deserialize(&buffer[..n]) {
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
                                        let response = ConsensusResponseType::ValidateTransaction(ValidateTransactionConsensusResponse {
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

    let addr = "127.0.0.1:50055";
    let consensus_service = ConsensusService::new().await;
    consensus_service.run(addr).await?;
    Ok(())
}
