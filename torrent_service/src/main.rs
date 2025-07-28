// torrent_service/src/main.rs
use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error};
use prometheus::{Counter, Gauge, Registry};
use std::num::NonZeroU32;
use std::sync::Arc;
use toml;
use governor::{Quota, RateLimiter};
use torrent_service::service::TorrentService;
use torrent_service::utils::ServiceError;

#[derive(Serialize, Deserialize, Debug)]
enum TorrentRequestType {
    OffloadAgedBlocks { token: String },
    GetProof { txid: String, block_hash: String, token: String },
    GetMetrics { token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum TorrentResponseType {
    OffloadSuccess { success: bool, error: String },
    ProofBundle { proof: String, error: String },
    Metrics { metrics: GetMetricsResponse },
}

#[derive(Serialize, Deserialize, Debug)]
struct GetMetricsResponse {
    service_name: String,
    requests_total: u64,
    avg_latency_ms: f64,
    errors_total: u64,
    alert_count: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let addr = "127.0.0.1:50062";
    let torrent_service = Arc::new(TorrentService::new().await);

    // Spawn background tasks
    let tracker_clone = Arc::clone(&torrent_service.tracker);
    tokio::spawn(async move {
        tracker_clone.run().await;
    });

    let proof_server_clone = Arc::clone(&torrent_service.proof_server);
    tokio::spawn(async move {
        proof_server_clone.run().await;
    });

    let aging_clone = Arc::clone(&torrent_service.aging);
    tokio::spawn(async move {
        aging_clone.run().await;
    });

    let listener = TcpListener::bind(addr).await?;
    info!("Torrent service listening on {}", addr);

    loop {
        let (mut stream, client_addr) = match listener.accept().await {
            Ok(stream) => stream,
            Err(e) => {
                error!("Failed to accept connection: {}", e);
                continue;
            }
        };

        let service_clone = Arc::clone(&torrent_service);
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer
            let n = match stream.read(&mut buffer).await {
                Ok(n) if n > 0 => n,
                Ok(_) => return, // Empty read
                Err(e) => {
                    error!("Read error from {}: {}", client_addr, e);
                    service_clone.errors_total.inc();
                    return;
                }
            };

            let request: TorrentRequestType = match deserialize(&buffer[..n]) {
                Ok(req) => req,
                Err(e) => {
                    error!("Deserialization error from {}: {}", client_addr, e);
                    service_clone.errors_total.inc();
                    let response = TorrentResponseType::OffloadSuccess {
                        success: false,
                        error: format!("Deserialization error: {}", e),
                    };
                    let encoded = serialize(&response).unwrap();
                    let _ = stream.write_all(&encoded).await;
                    let _ = stream.flush().await;
                    return;
                }
            };

            match service_clone.handle_request(request).await {
                Ok(response) => {
                    let encoded = serialize(&response).unwrap();
                    if let Err(e) = stream.write_all(&encoded).await {
                        error!("Write error to {}: {}", client_addr, e);
                        service_clone.errors_total.inc();
                    }
                    if let Err(e) = stream.flush().await {
                        error!("Flush error to {}: {}", client_addr, e);
                        service_clone.errors_total.inc();
                    }
                }
                Err(e) => {
                    error!("Request handling error from {}: {}", client_addr, e);
                    service_clone.errors_total.inc();
                    let response = TorrentResponseType::OffloadSuccess {
                        success: false,
                        error: e.to_string(),
                    };
                    let encoded = serialize(&response).unwrap();
                    if let Err(e) = stream.write_all(&encoded).await {
                        error!("Write error to {}: {}", client_addr, e);
                    }
                    if let Err(e) = stream.flush().await {
                        error!("Flush error to {}: {}", client_addr, e);
                        service_clone.errors_total.inc();
                    }
                }
            }
        });
    }
}
