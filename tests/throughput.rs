use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{serialize, deserialize};
use sv::messages::Tx;
use sv::util::Serializable;
use std::time::{Instant, Duration};
use serde::{Serialize, Deserialize};
use hex;

#[derive(Serialize, Deserialize, Debug)]
enum TransactionRequestType {
    ProcessTransaction { request: ProcessTransactionRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
struct ProcessTransactionRequest {
    tx_hex: String,
}

#[derive(Serialize, Deserialize, Debug)]
enum TransactionResponseType {
    ProcessTransaction(ProcessTransactionResponse),
}

#[derive(Serialize, Deserialize, Debug)]
struct ProcessTransactionResponse {
    success: bool,
    error: String,
}

#[tokio::test]
async fn test_throughput() {
    let addr = "127.0.0.1:50052"; // transaction_service address
    let token = "test_token".to_string(); // Replace with valid JWT token
    let tx = Tx::default(); // Create a dummy transaction
    let mut tx_bytes = Vec::new();
    tx.write(&mut tx_bytes).unwrap();
    let tx_hex = hex::encode(&tx_bytes);
    let request = TransactionRequestType::ProcessTransaction {
        request: ProcessTransactionRequest { tx_hex },
        token,
    };

    let mut total_requests = 0;
    let duration = Duration::from_secs(10); // Run for 10 seconds
    let start = Instant::now();

    while start.elapsed() < duration {
        let mut stream = match TcpStream::connect(addr).await {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("Skipping test: Failed to connect to transaction_service: {}", e);
                return; // Skip test if service is unavailable
            }
        };
        let encoded = serialize(&request).unwrap();
        if let Err(e) = stream.write_all(&encoded).await {
            eprintln!("Write error: {}", e);
            continue;
        }
        if let Err(e) = stream.flush().await {
            eprintln!("Flush error: {}", e);
            continue;
        }

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = match stream.read(&mut buffer).await {
            Ok(n) => n,
            Err(e) => {
                eprintln!("Read error: {}", e);
                continue;
            }
        };
        let response: TransactionResponseType = match deserialize(&buffer[..n]) {
            Ok(resp) => resp,
            Err(e) => {
                eprintln!("Deserialization error: {}", e);
                continue;
            }
        };

        match response {
            TransactionResponseType::ProcessTransaction(resp) => {
                assert!(resp.success, "Transaction failed: {}", resp.error);
            }
        }
        total_requests += 1;
    }

    let tps = total_requests as f64 / duration.as_secs_f64();
    println!("Throughput: {:.2} TPS", tps);
    assert!(tps >= 100.0, "Throughput below 100 TPS: {:.2}", tps);
}
