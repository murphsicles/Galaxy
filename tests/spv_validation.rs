use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{serialize, deserialize};
use sv::messages::{Tx, TxIn, TxOut, OutPoint};
use sv::util::{Serializable, Hash256};
use serde::{Serialize, Deserialize};
use hex;
use std::io::Cursor;

#[derive(Serialize, Deserialize, Debug)]
enum ValidationRequestType {
    GenerateSPVProof { request: GenerateSPVProofRequest, token: String },
    VerifySPVProof { request: VerifySPVProofRequest, token: String },
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationResponseType {
    GenerateSPVProof(GenerateSPVProofResponse),
    VerifySPVProof(VerifySPVProofResponse),
}

#[derive(Serialize, Deserialize, Debug)]
struct GenerateSPVProofRequest {
    txid: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct GenerateSPVProofResponse {
    success: bool,
    merkle_path: String,
    block_headers: Vec<String>,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct VerifySPVProofRequest {
    txid: String,
    merkle_path: String,
    block_headers: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct VerifySPVProofResponse {
    success: bool,
    error: String,
}

#[tokio::test]
async fn test_spv_proof_generation_and_verification() {
    let addr = "127.0.0.1:50057"; // validation_service address
    let token = "test_token".to_string(); // Replace with valid JWT token
    // Valid coinbase transaction from rust-sv tests (block 2 coinbase)
    let tx = Tx {
        version: 1,
        inputs: vec![TxIn {
            prev_output: OutPoint {
                hash: Hash256([0; 32]),
                index: 4294967295,
            },
            unlock_script: sv::script::Script(vec![4, 255, 255, 0, 29, 1, 11]),
            sequence: 4294967295,
        }],
        outputs: vec![TxOut {
            satoshis: 5000000000,
            lock_script: sv::script::Script(vec![
                65, 4, 114, 17, 168, 36, 245, 91, 80, 82, 40, 228, 195, 213, 25, 76, 31, 207,
                170, 21, 164, 86, 171, 223, 55, 249, 185, 217, 122, 64, 64, 175, 192, 115, 222,
                230, 200, 144, 100, 152, 79, 3, 56, 82, 55, 217, 33, 103, 193, 62, 35, 100, 70,
                180, 23, 171, 121, 160, 252, 174, 65, 42, 227, 49, 107, 119, 172,
            ]),
        }],
        lock_time: 0,
    };
    let mut tx_bytes = Vec::new();
    tx.write(&mut tx_bytes).unwrap();
    let txid = hex::encode(&tx_bytes);

    // Test GenerateSPVProof
    let generate_request = ValidationRequestType::GenerateSPVProof {
        request: GenerateSPVProofRequest { txid: txid.clone() },
        token: token.clone(),
    };
    let mut stream = match TcpStream::connect(addr).await {
        Ok(stream) => stream,
        Err(e) => panic!("Failed to connect to validation_service: {}", e),
    };
    let encoded = serialize(&generate_request).unwrap();
    stream.write_all(&encoded).await.unwrap();
    stream.flush().await.unwrap();

    let mut buffer = vec![0u8; 1024 * 1024];
    let n = stream.read(&mut buffer).await.unwrap();
    let response: ValidationResponseType = deserialize(&buffer[..n]).unwrap();

    let (merkle_path, block_headers) = match response {
        ValidationResponseType::GenerateSPVProof(resp) => {
            assert!(resp.success, "SPV proof generation failed: {}", resp.error);
            (resp.merkle_path, resp.block_headers)
        }
        _ => panic!("Unexpected response type"),
    };

    // Test VerifySPVProof
    let verify_request = ValidationRequestType::VerifySPVProof {
        request: VerifySPVProofRequest {
            txid,
            merkle_path,
            block_headers,
        },
        token,
    };
    let mut stream = match TcpStream::connect(addr).await {
        Ok(stream) => stream,
        Err(e) => panic!("Failed to connect to validation_service: {}", e),
    };
    let encoded = serialize(&verify_request).unwrap();
    stream.write_all(&encoded).await.unwrap();
    stream.flush().await.unwrap();

    let mut buffer = vec![0u8; 1024 * 1024];
    let n = stream.read(&mut buffer).await.unwrap();
    let response: ValidationResponseType = deserialize(&buffer[..n]).unwrap();

    match response {
        ValidationResponseType::VerifySPVProof(resp) => {
            assert!(resp.success, "SPV proof verification failed: {}", resp.error);
        }
        _ => panic!("Unexpected response type"),
    };
}
