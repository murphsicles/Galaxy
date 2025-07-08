use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bincode::{serialize, deserialize};
use sv::messages::Tx;
use sv::util::Serializable;
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
    let tx_hex = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff04deadbeef0100000001000000000000000000000000";
    let tx: Tx = Tx::read(&mut Cursor::new(hex::decode(tx_hex).unwrap())).unwrap();
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
