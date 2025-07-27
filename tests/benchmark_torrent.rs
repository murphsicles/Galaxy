// tests/benchmark_torrent.rs
use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use tokio::runtime::Runtime;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{deserialize, serialize};
use sv::messages::{Block, BlockHeader as Header, Tx as SvTx, OutPoint, MerkleBlock as MerklePath};
use sv::wallet::{PrivateKey, PublicKey};
use sv::ec::{Signature, Message};
use torrent_service::service::{TorrentService, GetAgedBlocksRequest, GetAgedBlocksResponse, TorrentRequestType, TorrentResponseType};
use torrent_service::utils::{Config, ServiceError, AgedThreshold};
use torrent_service::proof_server::{ProofBundle, ProofRequest, ProofResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

fn setup_mocks(rt: &Runtime) -> (Arc<TorrentService>, Vec<TcpListener>) {
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

    let torrent_service = rt.block_on(TorrentService::new_with_config(&config));
    let torrent_service = Arc::new(torrent_service);

    // Mock block_service
    let block_listener = rt.block_on(TcpListener::bind("127.0.0.1:50054")).unwrap();
    let block_service = Arc::clone(&torrent_service);
    rt.spawn(async move {
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
    let overlay_listener = rt.block_on(TcpListener::bind("127.0.0.1:50056")).unwrap();
    rt.spawn(async move {
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
    let validation_listener = rt.block_on(TcpListener::bind("127.0.0.1:50057")).unwrap();
    rt.spawn(async move {
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
    let transaction_listener = rt.block_on(TcpListener::bind("127.0.0.1:50052")).unwrap();
    rt.spawn(async move {
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
    let auth_listener = rt.block_on(TcpListener::bind("127.0.0.1:50060")).unwrap();
    rt.spawn(async move {
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
    let alert_listener = rt.block_on(TcpListener::bind("127.0.0.1:50061")).unwrap();
    rt.spawn(async move {
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
    let storage_listener = rt.block_on(TcpListener::bind("127.0.0.1:50053")).unwrap();
    rt.spawn(async move {
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
    let proof_listener = rt.block_on(TcpListener::bind("127.0.0.1:50063")).unwrap();
    rt.spawn(async move {
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

    // Mock backup node (for no-seeder fallback)
    let backup_listener = rt.block_on(TcpListener::bind("127.0.0.1:50064")).unwrap();
    rt.spawn(async move {
        loop {
            let (mut stream, addr) = backup_listener.accept().await.unwrap();
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

    (
        torrent_service,
        vec![
            block_listener,
            overlay_listener,
            validation_listener,
            transaction_listener,
            auth_listener,
            alert_listener,
            storage_listener,
            proof_listener,
            backup_listener,
        ],
    )
}

fn benchmark_torrent(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (torrent_service, _listeners) = setup_mocks(&rt);

    let mut group = c.benchmark_group("torrent_service");

    // Benchmark proof retrieval
    group.bench_function("proof_retrieval_high_tps", |b| {
        b.iter(|| {
            rt.block_on(async {
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
                assert!(matches!(response, TorrentResponseType::ProofBundle { .. }));
            });
        });
    });

    // Benchmark offload throughput
    group.bench_function("offload_throughput", |b| {
        b.iter(|| {
            rt.block_on(async {
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
                assert!(matches!(response, TorrentResponseType::OffloadSuccess { success: true, .. }));
            });
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_torrent);
criterion_main!(benches);
