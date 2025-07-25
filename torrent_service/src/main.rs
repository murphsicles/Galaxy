use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, warn, error};
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use toml;
use governor::{Quota, RateLimiter};
use torrent_service::service::TorrentService;
use torrent_service::utils::{Config, ServiceError};

#[derive(Serialize, Deserialize, Debug)]
enum TorrentRequestType {
    OffloadAgedBlocks { token: String },
    GetProof { txid: String, block_hash: String, token: String },
    GetMetrics { token: String },
    // Add more as needed, e.g., RegisterSeeder
}

#[derive(Serialize, Deserialize, Debug)]
enum TorrentResponseType {
    OffloadSuccess { success: bool, error: String },
    ProofBundle { proof: String /* Serialized bundle */, error: String },
    Metrics { metrics: GetMetricsResponse },
}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsResponse {
    service_name: String,
    requests_total: u64,
    avg_latency_ms: f64,
    errors_total: u64,
    alert_count: u64,
    // Add torrent-specific: seeder_count, torrent_count
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "127.0.0.1:50055";  // Choose a unique port
    let torrent_service = Arc::new(TorrentService::new().await);
    let listener = TcpListener::bind(addr).await?;
    info!("Torrent service running on {}", addr);

    loop {
        match listener.accept().await {
            Ok((mut stream, _)) => {
                let service = torrent_service.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 1024 * 1024];
                    match stream.read(&mut buffer).await {
                        Ok(n) => {
                            let request: TorrentRequestType = match deserialize(&buffer[..n]) {
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
                                    // Send error response
                                    let response = TorrentResponseType::OffloadSuccess {
                                        success: false,
                                        error: e.to_string(),
                                    };
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
