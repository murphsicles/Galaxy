// tests/torrent_service.rs
use bincode::{deserialize, serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, Duration};
use tracing::info;
use sv::block::{Block, Header};
use sv::transaction::Transaction as SvTx;
use sv::merkle::MerklePath;
use shared::AgedThreshold;
use torrent_service::service::{TorrentService, GetAgedBlocksRequest, GetAgedBlocksResponse};
use torrent_service::utils::ServiceError;
use torrent_service::proof_server::ProofBundle;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Debug)]
enum MockRequestType {
    GetAgedBlocks(GetAgedBlocksRequest),
    StoreTorrentRef(StoreTorrentRefRequest),
    ValidateProof(ValidateProofRequest),
    BroadcastTx(BroadcastTxRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum MockResponseType {
    GetAgedBlocks(GetAgedBlocksResponse),
    StoreTorrentRef(StoreTorrentRefResponse),
    ValidateProof(ValidateProofResponse),
    BroadcastTx(BroadcastTxResponse),
}

#[derive(Serialize, Deserialize, Debug)]
struct StoreTorrentRefRequest {
    info_hash: String,
    block_hashes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct StoreTorrentRefResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofRequest {
    proof: ProofBundle,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxRequest {
    tx: SvTx,
    token: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct BroadcastTxResponse {
    success: bool,
    error: String,
}

#[tokio::test]
async fn test_torrent_service_end_to_end() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Mock block_service
    let block_listener = TcpListener::bind("127.0.0.1:50054").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = block_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetAgedBlocks(req) => {
                    let block = Block {
                        header: Header {
                            timestamp: 1234567890, // Old timestamp
                            ..Default::default()
                        },
                        transactions: vec![SvTx::default()],
                        ..Default::default()
                    };
                    let resp = GetAgedBlocksResponse {
                        blocks: vec![block],
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::GetAgedBlocks(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    });

    // Mock overlay_service
    let overlay_listener = TcpListener::bind("127.0.0.1:50056").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = overlay_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::StoreTorrentRef(_) => {
                    let resp = StoreTorrentRefResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::StoreTorrentRef(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    });

    // Mock validation_service
    let validation_listener = TcpListener::bind("127.0.0.1:50057").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = overlay_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::ValidateProof(_) => {
                    let resp = ValidateProofResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::ValidateProof(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    });

    // Mock transaction_service
    let transaction_listener = TcpListener::bind("127.0.0.1:50052").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = transaction_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::BroadcastTx(_) => {
                    let resp = BroadcastTxResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::BroadcastTx(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => panic!("Unexpected request"),
            }
        }
    });

    // Mock auth_service
    let auth_listener = TcpListener::bind("127.0.0.1:50060").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = auth_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: AuthRequest = deserialize(&buffer[..n]).unwrap();
            let resp = AuthResponse {
                success: true,
                user_id: "test_user".to_string(),
                error: String::new(),
            };
            let encoded = serialize(&resp).unwrap();
            stream.write_all(&encoded).await.unwrap();
            stream.flush().await.unwrap();
        }
    });

    // Mock alert_service
    let alert_listener = TcpListener::bind("127.0.0.1:50061").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = alert_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: AlertRequest = deserialize(&buffer[..n]).unwrap();
            let resp = AlertResponse {
                success: true,
                error: String::new(),
            };
            let encoded = serialize(&resp).unwrap();
            stream.write_all(&encoded).await.unwrap();
            stream.flush().await.unwrap();
        }
    });

    // Initialize torrent_service
    let torrent_service = Arc::new(TorrentService::new().await);

    // Wait for aging to detect blocks
    sleep(Duration::from_secs(1)).await;

    // Simulate proof request
    let mut stream = TcpStream::connect("127.0.0.1:50062").await.unwrap();
    let request = TorrentRequestType::GetProof {
        txid: "dummy_txid".to_string(),
        block_hash: "dummy_block_hash".to_string(),
        token: "dummy_token".to_string(),
    };
    let encoded = serialize(&request).unwrap();
    stream.write_all(&encoded).await.unwrap();
    stream.flush().await.unwrap();

    let mut buffer = vec![0u8; 1024 * 1024];
    let n = stream.read(&mut buffer).await.unwrap();
    let response: TorrentResponseType = deserialize(&buffer[..n]).unwrap();

    match response {
        TorrentResponseType::ProofBundle { proof, error } => {
            assert!(error.is_empty(), "Proof request failed: {}", error);
            assert!(!proof.is_empty(), "Proof is empty");
            info!("Successfully retrieved proof: {}", proof);
        }
        _ => panic!("Unexpected response type"),
    }
}

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
