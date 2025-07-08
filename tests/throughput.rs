use tokio::net::TcpStream;
use bincode::{serialize, deserialize};
use sv::transaction::Transaction;
use sv::util::serialize as sv_serialize;
use std::time::{Instant, Duration};

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum TransactionRequestType {
    ProcessTransaction { request: ProcessTransactionRequest, token: String },
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ProcessTransactionRequest {
    tx_hex: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
enum TransactionResponseType {
    ProcessTransaction(ProcessTransactionResponse),
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ProcessTransactionResponse {
    success: bool,
    error: String,
}

#[tokio::test]
async fn test_throughput() {
    let addr = "127.0.0.1:50052"; // transaction_service address
    let token = "test_token".to_string(); // Replace with valid JWT token
    let tx = Transaction::new(); // Create a dummy transaction
    let tx_hex = hex::encode(sv_serialize(&tx).unwrap());
    let request = TransactionRequestType::ProcessTransaction {
        request: ProcessTransactionRequest { tx_hex },
        token,
    };

    let mut total_requests = 0;
    let duration = Duration::from_secs(10); // Run for 10 seconds
    let start = Instant::now();

    while start.elapsed() < duration {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let encoded = serialize(&request).unwrap();
        stream.write_all(&encoded).await.unwrap();
        stream.flush().await.unwrap();

        let mut buffer = vec![0u8; 1024 * 1024];
        let n = stream.read(&mut buffer).await.unwrap();
        let response: TransactionResponseType = deserialize(&buffer[..n]).unwrap();

        match response {
            TransactionResponseType::ProcessTransaction(resp) => {
                assert!(resp.success, "Transaction failed: {}", resp.error);
            }
        }
        total_requests += 1;
    }

    let tps = total_requests as f64 / duration.as_secs_f64();
    println!("Throughput: {:.2} TPS", tps);
    assert!(tps >= 100_000_000.0, "Throughput below 100M TPS: {:.2}", tps);
}
