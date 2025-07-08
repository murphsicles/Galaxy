use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{info, error};

#[derive(Serialize, Deserialize, Debug)]
struct Transaction {
    id: String,
    amount: u64,
    timestamp: u64,
}

async fn handle_client(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Simulate batching 1,000 transactions
    let txs: Vec<Transaction> = (0..1_000)
        .map(|i| Transaction {
            id: format!("tx{}", i),
            amount: 100,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        })
        .collect();
    
    // Serialize with bincode
    let encoded = serialize(&txs)?;
    info!("Sending {} transactions, {} bytes", txs.len(), encoded.len());
    
    // Write to stream
    stream.write_all(&encoded).await?;
    stream.flush().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    info!("Shared service running on 127.0.0.1:8080");
    
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                info!("New connection from {}", addr);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream).await {
                        error!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => error!("Accept error: {}", e),
        }
    }
}
