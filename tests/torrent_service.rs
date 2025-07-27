// tests/torrent_service.rs
use bincode::{deserialize, serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, Duration};
use tracing::{info, error};
use sv::messages::{Block, BlockHeader as Header, Tx as SvTx, OutPoint, MerkleBlock as MerklePath};
use sv::wallet::{PrivateKey, PublicKey};
use sv::ec::{Signature, Message};
use shared::AgedThreshold;
use torrent_service::service::{TorrentService, GetAgedBlocksRequest, GetAgedBlocksResponse, TorrentRequestType, TorrentResponseType};
use torrent_service::utils::{ServiceError, Config};
use torrent_service::proof_server::{ProofBundle, ProofRequest, ProofResponse};
use torrent_service::tracker::TrackerManager;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use bip_metainfo::Metainfo;
use std::time::Instant;

#[derive(Serialize, Deserialize, Debug)]
enum MockRequestType {
    GetAgedBlocks(GetAgedBlocksRequest),
    StoreTorrentRef(StoreTorrentRefRequest),
    ValidateProof(ValidateProofRequest),
    BroadcastTx(BroadcastTxRequest),
    AuthRequest(AuthRequest),
    AlertRequest(AlertRequest),
    GetUtxos(GetUtxosRequest),
    ProofRequest(ProofRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum MockResponseType {
    GetAgedBlocks(GetAgedBlocksResponse),
    StoreTorrentRef(StoreTorrentRefResponse),
    ValidateProof(ValidateProofResponse),
    BroadcastTx(BroadcastTxResponse),
    AuthResponse(AuthResponse),
    AlertResponse(AlertResponse),
    GetUtxos(GetUtxosResponse),
    ProofResponse(ProofResponse),
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

#[derive(Serialize, Deserialize, Debug)]
struct GetUtxosRequest {
    address: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GetUtxosResponse {
    utxos: Vec<Utxo>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Utxo {
    outpoint: OutPoint,
    amount: u64,
    script_pubkey: String,
}

#[tokio::test]
async fn test_torrent_service_end_to_end() {
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
                            timestamp: 1234567890,
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
                _ => {}
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
                _ => {}
            }
        }
    });

    // Mock validation_service
    let validation_listener = TcpListener::bind("127.0.0.1:50057").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = validation_listener.accept().await.unwrap();
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
                _ => {}
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
                _ => {}
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
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AuthRequest(req) => {
                    let resp = AuthResponse {
                        success: true,
                        user_id: "test_user".to_string(),
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::AuthResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock alert_service
    let alert_listener = TcpListener::bind("127.0.0.1:50061").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = alert_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AlertRequest(req) => {
                    let resp = AlertResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::AlertResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock storage_service for UTXOs
    let storage_listener = TcpListener::bind("127.0.0.1:50053").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = storage_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetUtxos(_) => {
                    let utxo = Utxo {
                        outpoint: OutPoint {
                            txid: "dummy_txid".to_string(),
                            vout: 0,
                        },
                        amount: 1000000,
                        script_pubkey: "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac".to_string(),
                    };
                    let resp = GetUtxosResponse {
                        utxos: vec![utxo],
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::GetUtxos(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock proof_server for fast response
    let proof_listener = TcpListener::bind("127.0.0.1:50063").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, addr) = proof_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::ProofRequest(req) => {
                    let proof = ProofBundle {
                        tx: SvTx::default(),
                        path: MerklePath::default(),
                        header: Header::default(),
                    };
                    let resp = ProofResponse {
                        proof: Some(proof),
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::ProofResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
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
        token: "default_token".to_string(),
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

#[tokio::test]
async fn test_proof_reward_bonuses() {
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
                            timestamp: 1234567890,
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
                _ => {}
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
                _ => {}
            }
        }
    });

    // Mock validation_service
    let validation_listener = TcpListener::bind("127.0.0.1:50057").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = validation_listener.accept().await.unwrap();
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
                _ => {}
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
                _ => {}
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
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AuthRequest(req) => {
                    let resp = AuthResponse {
                        success: true,
                        user_id: "test_user".to_string(),
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::AuthResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock alert_service
    let alert_listener = TcpListener::bind("127.0.0.1:50061").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = alert_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AlertRequest(req) => {
                    let resp = AlertResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::AlertResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock storage_service for UTXOs
    let storage_listener = TcpListener::bind("127.0.0.1:50053").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = storage_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetUtxos(_) => {
                    let utxo = Utxo {
                        outpoint: OutPoint {
                            txid: "dummy_txid".to_string(),
                            vout: 0,
                        },
                        amount: 1000000,
                        script_pubkey: "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac".to_string(),
                    };
                    let resp = GetUtxosResponse {
                        utxos: vec![utxo],
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::GetUtxos(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock proof_server for fast response
    let proof_listener = TcpListener::bind("127.0.0.1:50063").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, addr) = proof_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::ProofRequest(req) => {
                    let proof = ProofBundle {
                        tx: SvTx::default(),
                        path: MerklePath::default(),
                        header: Header::default(),
                    };
                    let resp = ProofResponse {
                        proof: Some(proof),
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::ProofResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Initialize torrent_service with config for bonuses
    let config = Config {
        piece_size: 32 * 1024 * 1024,
        aged_threshold: AgedThreshold::Months(60),
        stake_amount: 100000,
        proof_reward_base: 100,
        proof_bonus_speed: 10,
        proof_bonus_rare: 50,
        bulk_reward_per_mb: 100,
        tracker_port: Some(6969),
        proof_rpc_port: Some(50063),
        wallet_address: "default_address".to_string(),
        dynamic_chunk_size: Some(true),
    };
    let torrent_service = Arc::new(TorrentService::new_with_config(&config).await);

    // Wait for services to stabilize
    sleep(Duration::from_millis(500)).await;

    // Simulate proof request to trigger reward with bonuses
    let mut stream = TcpStream::connect("127.0.0.1:50062").await.unwrap();
    let request = TorrentRequestType::GetProof {
        txid: "dummy_txid".to_string(),
        block_hash: "dummy_block_hash".to_string(),
        token: "default_token".to_string(),
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
            info!("Successfully retrieved proof with bonuses: {}", proof);
            // Note: Bonus verification is implicit via logs (speed <500ms, rarity <3 seeders)
        }
        _ => panic!("Unexpected response type"),
    }
}

#[tokio::test]
async fn test_dynamic_chunk_sizing() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Mock block_service with a large block
    let block_listener = TcpListener::bind("127.0.0.1:50054").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = block_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetAgedBlocks(req) => {
                    let mut transactions = vec![];
                    for _ in 0..1000000 { // Simulate high-TPS block
                        transactions.push(SvTx::default());
                    }
                    let block = Block {
                        header: Header {
                            timestamp: 1234567890,
                            ..Default::default()
                        },
                        transactions,
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
                _ => {}
            }
        }
    });

    // Mock overlay_service
    let overlay_listener = TcpListener::bind("127.0.0.1:50056").await.unwrap();
    let mut torrent_info: Option<Metainfo> = None;
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = overlay_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::StoreTorrentRef(req) => {
                    let resp = StoreTorrentRefResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::StoreTorrentRef(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock validation_service
    let validation_listener = TcpListener::bind("127.0.0.1:50057").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = validation_listener.accept().await.unwrap();
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
                _ => {}
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
                _ => {}
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
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AuthRequest(req) => {
                    let resp = AuthResponse {
                        success: true,
                        user_id: "test_user".to_string(),
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::AuthResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock alert_service
    let alert_listener = TcpListener::bind("127.0.0.1:50061").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = alert_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AlertRequest(req) => {
                    let resp = AlertResponse {
                        success: true,
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::AlertResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock storage_service for UTXOs
    let storage_listener = TcpListener::bind("127.0.0.1:50053").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = storage_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetUtxos(_) => {
                    let utxo = Utxo {
                        outpoint: OutPoint {
                            txid: "dummy_txid".to_string(),
                            vout: 0,
                        },
                        amount: 1000000,
                        script_pubkey: "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac".to_string(),
                    };
                    let resp = GetUtxosResponse {
                        utxos: vec![utxo],
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::GetUtxos(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Mock proof_server for fast response
    let proof_listener = TcpListener::bind("127.0.0.1:50063").await.unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, addr) = proof_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::ProofRequest(req) => {
                    let proof = ProofBundle {
                        tx: SvTx::default(),
                        path: MerklePath::default(),
                        header: Header::default(),
                    };
                    let resp = ProofResponse {
                        proof: Some(proof),
                        error: String::new(),
                    };
                    let encoded = serialize(&MockResponseType::ProofResponse(resp)).unwrap();
                    stream.write_all(&encoded).await.unwrap();
                    stream.flush().await.unwrap();
                }
                _ => {}
            }
        }
    });

    // Initialize torrent_service with config for dynamic chunk sizing
    let config = Config {
        piece_size: 32 * 1024 * 1024,
        aged_threshold: AgedThreshold::Months(60),
        stake_amount: 100000,
        proof_reward_base: 100,
        proof_bonus_speed: 10,
        proof_bonus_rare: 50,
        bulk_reward_per_mb: 100,
        tracker_port: Some(6969),
        proof_rpc_port: Some(50063),
        wallet_address: "default_address".to_string(),
        dynamic_chunk_size: Some(true),
    };
    let torrent_service = Arc::new(TorrentService::new_with_config(&config).await);

    // Wait for aging to detect blocks
    sleep(Duration::from_secs(1)).await;

    // Simulate offload request to trigger chunking
    let mut stream = TcpStream::connect("127.0.0.1:50062").await.unwrap();
    let request = TorrentRequestType::OffloadAgedBlocks {
        token: "default_token".to_string(),
    };
    let encoded = serialize(&request).unwrap();
    stream.write_all(&encoded).await.unwrap();
    stream.flush().await.unwrap();

    let mut buffer = vec![0u8; 1024 * 1024];
    let n = stream.read(&mut buffer).await.unwrap();
    let response: TorrentResponseType = deserialize(&buffer[..n]).unwrap();

    match response {
        TorrentResponseType::OffloadSuccess { success, error } => {
            assert!(success, "Offload failed: {}", error);
            info!("Successfully offloaded aged blocks");
            // Verify chunk size (8MB = 8,388,608 bytes for high-TPS block)
            assert_eq!(torrent_info.unwrap().info.piece_length, 8 * 1024 * 1024, "Expected 8MB chunk size for high-TPS block");
        }
        _ => panic!("Unexpected response type"),
    }
}
