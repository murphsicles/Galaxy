// tests/benchmark_torrent.rs
use criterion::{criterion_group, criterion_main, Criterion};
use tokio::runtime::Runtime;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use serde::{Deserialize, Serialize};
use tracing_subscriber;

// Mock torrent_service dependencies
#[cfg(test)]
mod torrent_service {
    pub mod service {
        use serde::{Deserialize, Serialize};

        #[derive(Clone, Debug)]
        pub struct TorrentService;

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub struct GetAgedBlocksRequest;

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub struct GetAgedBlocksResponse {
            pub blocks: Vec<crate::Block>,
            pub error: String,
        }

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub enum TorrentRequestType {
            GetProof {
                txid: String,
                block_hash: String,
                token: String,
            },
        }

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub enum TorrentResponseType {
            ProofBundle {
                proof: Vec<String>,
                error: String,
            },
        }

        impl TorrentService {
            pub async fn new_with_config(_config: &Config) -> Self {
                TorrentService
            }
        }

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub struct Config {
            pub piece_size: u64,
            pub aged_threshold: AgedThreshold,
            pub stake_amount: u64,
            pub proof_reward_base: u64,
            pub proof_bonus_speed: u64,
            pub proof_bonus_rare: u64,
            pub bulk_reward_per_mb: u64,
            pub tracker_port: Option<u16>,
            pub proof_rpc_port: Option<u16>,
            pub wallet_address: String,
            pub dynamic_chunk_size: Option<bool>,
        }

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub enum AgedThreshold {
            Months(u32),
        }
    }

    pub mod proof_server {
        use serde::{Deserialize, Serialize};
        use crate::BlockHeader;

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub struct ProofBundle {
            pub tx_hex: String,
            pub path: Vec<String>,
            pub header: BlockHeader,
        }

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub struct ProofRequest;

        #[derive(Clone, Serialize, Deserialize, Debug)]
        pub struct ProofResponse {
            pub proof: Option<ProofBundle>,
            pub error: String,
        }
    }
}

// Mock sv types
#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct Block {
    header: BlockHeader,
    txns: Vec<SvTx>,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct BlockHeader {
    timestamp: u32,
}

#[derive(Clone, Default, Serialize, Deserialize, Debug)]
struct SvTx;

#[derive(Serialize, Deserialize, Debug)]
enum MockRequestType {
    GetAgedBlocks(torrent_service::service::GetAgedBlocksRequest),
    StoreTorrentRef(StoreTorrentRefRequest),
    ValidateProof(ValidateProofRequest),
    BroadcastTx(BroadcastTxRequest),
    AuthRequest(AuthRequest),
    AlertRequest(AlertRequest),
    GetUtxos(GetUtxosRequest),
    ProofRequest(torrent_service::proof_server::ProofRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum MockResponseType {
    GetAgedBlocks(torrent_service::service::GetAgedBlocksResponse),
    StoreTorrentRef(StoreTorrentRefResponse),
    ValidateProof(ValidateProofResponse),
    BroadcastTx(BroadcastTxResponse),
    AuthResponse(AuthResponse),
    AlertResponse(AlertResponse),
    GetUtxos(GetUtxosResponse),
    ProofResponse(torrent_service::proof_server::ProofResponse),
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
    proof: torrent_service::proof_server::ProofBundle,
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

fn setup_mocks(rt: &Runtime) -> torrent_service::service::TorrentService {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let config = torrent_service::service::Config {
        piece_size: 32 * 1024 * 1024,
        aged_threshold: torrent_service::service::AgedThreshold::Months(60),
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
    let torrent_service = rt.block_on(torrent_service::service::TorrentService::new_with_config(&config));

    // Mock block_service
    let block_listener = rt.block_on(TcpListener::bind("127.0.0.1:50054")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = block_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetAgedBlocks(_req) => {
                    let block = Block {
                        header: BlockHeader {
                            timestamp: 1234567890,
                            ..Default::default()
                        },
                        txns: vec![SvTx::default(); 1000],
                        ..Default::default()
                    };
                    let resp = torrent_service::service::GetAgedBlocksResponse {
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
    let _overlay_listener = rt.block_on(TcpListener::bind("127.0.0.1:50056")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = _overlay_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::StoreTorrentRef(_req) => {
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
    let validation_listener = rt.block_on(TcpListener::bind("127.0.0.1:50057")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = validation_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::ValidateProof(_req) => {
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
    let transaction_listener = rt.block_on(TcpListener::bind("127.0.0.1:50052")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = transaction_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::BroadcastTx(_req) => {
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
    let auth_listener = rt.block_on(TcpListener::bind("127.0.0.1:50060")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = auth_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AuthRequest(_req) => {
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
    let alert_listener = rt.block_on(TcpListener::bind("127.0.0.1:50061")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = alert_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::AlertRequest(_req) => {
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

    // Mock storage_service
    let storage_listener = rt.block_on(TcpListener::bind("127.0.0.1:50053")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _) = storage_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::GetUtxos(_req) => {
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

    // Mock proof_server
    let proof_listener = rt.block_on(TcpListener::bind("127.0.0.1:50063")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, _addr) = proof_listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 1024 * 1024];
            let n = stream.read(&mut buffer).await.unwrap();
            let req: MockRequestType = deserialize(&buffer[..n]).unwrap();
            match req {
                MockRequestType::ProofRequest(_req) => {
                    let proof = torrent_service::proof_server::ProofBundle {
                        tx_hex: "dummy_tx_hex".to_string(),
                        path: vec![],
                        header: BlockHeader::default(),
                    };
                    let resp = torrent_service::proof_server::ProofResponse {
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

    torrent_service
}

fn benchmark_torrent(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let _torrent_service = setup_mocks(&rt);

    let mut group = c.benchmark_group("torrent_service");

    // Benchmark proof retrieval
    group.bench_function("proof_retrieval_high_tps", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut stream = TcpStream::connect("127.0.0.1:50062").await.unwrap();
                let request = torrent_service::service::TorrentRequestType::GetProof {
                    txid: "dummy_txid".to_string(),
                    block_hash: "dummy_block_hash".to_string(),
                    token: "default_token".to_string(),
                };
                let encoded = serialize(&request).unwrap();
                stream.write_all(&encoded).await.unwrap();
                stream.flush().await.unwrap();

                let mut buffer = vec![0u8; 1024 * 1024];
                let n = stream.read(&mut buffer).await.unwrap();
                let response: torrent_service::service::TorrentResponseType = deserialize(&buffer[..n]).unwrap();
                assert!(matches!(response, torrent_service::service::TorrentResponseType::ProofBundle { .. }));
            });
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_torrent);
criterion_main!(benches);
