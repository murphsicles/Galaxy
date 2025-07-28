// tests/torrent_service.rs
use bincode::{deserialize, serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, Duration};
use tracing::info;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Mock structures to replace unavailable dependencies
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Block {
    header: Header,
    txns: Vec<SvTx>,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Header {
    timestamp: u32,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct SvTx;

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct OutPoint {
    hash: String,
    index: u32,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct MerklePath;

#[derive(Clone, Serialize, Deserialize, Debug)]
struct TorrentService {
    tracker: Arc<TrackerManager>,
    incentives: Arc<Incentives>,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct TrackerManager {
    reputation: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Reputation>>>,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Incentives;

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Reputation {
    score: i32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct GetAgedBlocksRequest;

#[derive(Clone, Serialize, Deserialize, Debug)]
struct GetAgedBlocksResponse {
    blocks: Vec<Block>,
    error: String,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
enum TorrentRequestType {
    GetProof {
        txid: String,
        block_hash: String,
        token: String,
    },
}

#[derive(Clone, Serialize, Deserialize, Debug)]
enum TorrentResponseType {
    ProofBundle {
        proof: Vec<String>,
        error: String,
    },
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct ProofBundle {
    tx_hex: String,
    path: Vec<String>,
    header: Header,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
struct ProofRequest;

#[derive(Clone, Serialize, Deserialize, Debug)]
struct ProofResponse {
    proof: Option<ProofBundle>,
    error: String,
}

impl TorrentService {
    async fn new() -> Self {
        TorrentService {
            tracker: Arc::new(TrackerManager {
                reputation: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            }),
            incentives: Arc::new(Incentives),
        }
    }
}

impl TrackerManager {
    async fn register_seeder(
        &self,
        peer_id: &str,
        _info_hash: &str,
        _signature: &Signature,
        _message: &Message,
        _pub_key: &PublicKey,
    ) -> Result<(), String> {
        let rep = self.reputation.lock().await;
        if rep.get(peer_id).map_or(0, |r| r.score) < 50 {
            Err("Insufficient reputation for seeder registration".to_string())
        } else {
            Ok(())
        }
    }
}

impl Incentives {
    async fn stake(&self, peer_id: &str, amount: u64) -> Result<(), String> {
        Ok(())
    }

    async fn reward_proof(&self, peer_id: &str, _info_hash: &str) -> Result<(), String> {
        Ok(())
    }

    async fn reward_bulk(&self, peer_id: &str, _mb: u64) -> Result<(), String> {
        Ok(())
    }

    async fn slash(&self, peer_id: &str, _amount: u64) -> Result<(), String> {
        Ok(())
    }
}

// Mock types for removed dependencies
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct PrivateKey;

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct PublicKey;

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Signature;

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Message;

impl PrivateKey {
    fn from_random() -> Self {
        PrivateKey
    }
}

impl PublicKey {
    fn from_private_key(_priv_key: &PrivateKey) -> Self {
        PublicKey
    }
}

impl Signature {
    fn sign(_priv_key: &PrivateKey, _message: &Message) -> Self {
        Signature
    }
}

impl Message {
    fn from_str(_s: &str) -> Result<Self, String> {
        Ok(Message)
    }
}

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
    tx_hex: String,
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
    txid: String,
    vout: u32,
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
                        txns: vec![SvTx::default()],
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
                        txid: "dummy_txid".to_string(),
                        vout: 0,
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
                        tx_hex: "dummy_tx_hex".to_string(),
                        path: vec![],
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
            info!("Successfully retrieved proof: {:?}", proof);
        }
        _ => { panic!("Unexpected response type"); }
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
                    let mut txns = vec![];
                    for _ in 0..1000 {
                        txns.push(SvTx::default());
                    }
                    let block = Block {
                        header: Header {
                            timestamp: 1234567890,
                            ..Default::default()
                        },
                        txns,
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
                        txid: "dummy_txid".to_string(),
                        vout: 0,
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
                        tx_hex: "dummy_tx_hex".to_string(),
                        path: vec![],
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
            info!("Successfully retrieved proof: {:?}", proof);
        }
        _ => { panic!("Unexpected response type"); }
    }
}

#[tokio::test]
async fn test_proof_reward_bonuses() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let torrent_service = Arc::new(TorrentService::new().await);
    let tracker = torrent_service.tracker.clone();
    let incentives = torrent_service.incentives.clone();

    // Test initial registration failure (low reputation)
    let priv_key = PrivateKey::from_random();
    let pub_key = PublicKey::from_private_key(&priv_key);
    let message = Message::from_str("test_message").unwrap();
    let signature = Signature::sign(&priv_key, &message).unwrap();
    let peer_id = "test_peer";
    let info_hash = "dummy_info_hash";
    let err = tracker.register_seeder(peer_id, info_hash, &signature, &message, &pub_key).await.err().unwrap();
    assert_eq!(err.to_string(), "Insufficient reputation for seeder registration");

    // Simulate stake to gain reputation (+100 points)
    incentives.stake(peer_id, 100000).await.unwrap();

    // Test successful registration after stake
    tracker.register_seeder(peer_id, info_hash, &signature, &message, &pub_key).await.unwrap();

    // Simulate reward to gain more reputation (+10 points)
    incentives.reward_proof(peer_id, info_hash).await.unwrap();

    // Simulate bulk reward to gain more reputation (+5/MB)
    incentives.reward_bulk(peer_id, 2).await.unwrap();

    // Simulate slash to reduce reputation (-50 points per 100,000 sat)
    incentives.slash(peer_id, 100000).await.unwrap();

    // Verify reputation is updated correctly (initial 0 +100 stake +10 proof +10 bulk (2MB *5) -50 slash = 70)
    let rep = tracker.reputation.lock().await.get(peer_id).unwrap();
    assert_eq!(rep.score, 70);
}
