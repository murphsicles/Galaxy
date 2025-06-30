use tonic::{transport::{Server, Channel}, Request, Response, Status};
use validation::validation_server::{Validation, ValidationServer};
use validation::{
    GenerateSPVProofRequest, GenerateSPVProofResponse,
    VerifySPVProofRequest, VerifySPVProofResponse,
    BatchGenerateSPVProofRequest, BatchGenerateSPVProofResponse
};
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use sv::block::Block;
use sv::util::{deserialize, serialize, hash::Sha256d};
use hex;
use std::collections::VecDeque;

tonic::include_proto!("validation");
tonic::include_proto!("block");
tonic::include_proto!("storage");

#[derive(Debug)]
struct ValidationServiceImpl {
    block_client: BlockClient<Channel>,
    storage_client: StorageClient<Channel>,
}

impl ValidationServiceImpl {
    async fn new() -> Self {
        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        ValidationServiceImpl { block_client, storage_client }
    }

    async fn generate_merkle_path(&self, txid: &str, block: &Block) -> Result<Vec<u8>, String> {
        // Calculate merkle path
        let txids: Vec<Sha256d> = block.transactions.iter()
            .map(|tx| tx.txid())
            .collect();
        let target_txid = Sha256d::from_hex(txid)
            .map_err(|e| format!("Invalid txid: {}", e))?;

        let mut path = vec![];
        let mut current_level = txids;
        let mut index = current_level.iter().position(|id| *id == target_txid)
            .ok_or_else(|| "Transaction not found in block".to_string())?;

        while current_level.len() > 1 {
            let mut next_level = vec![];
            for i in (0..current_level.len()).step_by(2) {
                let left = current_level[i];
                let right = if i + 1 < current_level.len() {
                    current_level[i + 1]
                } else {
                    left // Duplicate last hash if odd number of txids
                };
                if i == index || i + 1 == index {
                    path.extend_from_slice(if i == index { &right[..] } else { &left[..] });
                    index = i / 2;
                }
                let mut combined = [0u8; 64];
                combined[..32].copy_from_slice(&left[..]);
                combined[32..].copy_from_slice(&right[..]);
                let parent = Sha256d::double_sha256(&combined);
                next_level.push(parent);
            }
            current_level = next_level;
        }

        Ok(path)
    }

    async fn validate_difficulty(&self, header: &sv::block::BlockHeader) -> Result<bool, String> {
        // TODO: Retrieve target difficulty from blockchain state (e.g., via block_service)
        let target_bits = 0x1d00ffff; // Placeholder difficulty (mainnet genesis)
        let target = Self::bits_to_target(target_bits);
        let hash = header.hash();
        Ok(hash <= target)
    }

    fn bits_to_target(bits: u32) -> [u8; 32] {
        // Convert compact difficulty bits to target
        let exponent = (bits >> 24) as u8;
        let mantissa = bits & 0x007fffff;
        let mut target = [0u8; 32];
        if exponent <= 3 {
            let value = mantissa >> (8 * (3 - exponent));
            target[31] = (value & 0xff) as u8;
            target[30] = ((value >> 8) & 0xff) as u8;
            target[29] = ((value >> 16) & 0xff) as u8;
        } else {
            let offset = 32 - exponent as usize;
            target[offset] = (mantissa & 0xff) as u8;
            target[offset + 1] = ((mantissa >> 8) & 0xff) as u8;
            target[offset + 2] = ((mantissa >> 16) & 0xff) as u8;
        }
        target
    }
}

#[tonic::async_trait]
impl Validation for ValidationServiceImpl {
    async fn generate_spv_proof(&self, request: Request<GenerateSPVProofRequest>) -> Result<Response<GenerateSPVProofResponse>, Status> {
        let req = request.into_inner();
        // TODO: Query block_service for block containing txid
        // Placeholder: Assume a dummy block
        let block = Block {
            header: sv::block::BlockHeader {
                version: 1,
                prev_blockhash: Default::default(),
                merkle_root: Sha256d::from_hex("0000000000000000000000000000000000000000000000000000000000000000")
                    .unwrap(),
                time: 0,
                bits: 0x1d00ffff,
                nonce: 0,
            },
            transactions: vec![], // TODO: Fetch real transactions
        };

        let merkle_path = self.generate_merkle_path(&req.txid, &block)
            .await
            .map_err(|e| Status::internal(format!("Failed to generate merkle path: {}", e)))?;
        let block_headers = vec![hex::encode(serialize(&block.header))];

        let reply = GenerateSPVProofResponse {
            success: true,
            merkle_path: hex::encode(merkle_path),
            block_headers,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn verify_spv_proof(&self, request: Request<VerifySPVProofRequest>) -> Result<Response<VerifySPVProofResponse>, Status> {
        let req = request.into_inner();
        let merkle_path = hex::decode(&req.merkle_path)
            .map_err(|e| Status::invalid_argument(format!("Invalid merkle_path: {}", e)))?;
        let txid = Sha256d::from_hex(&req.txid)
            .map_err(|e| Status::invalid_argument(format!("Invalid txid: {}", e)))?;

        // Verify block headers
        let mut prev_hash = None;
        for header_hex in req.block_headers {
            let header_bytes = hex::decode(&header_hex)
                .map_err(|e| Status::invalid_argument(format!("Invalid block header: {}", e)))?;
            let header: sv::block::BlockHeader = deserialize(&header_bytes)
                .map_err(|e| Status::invalid_argument(format!("Invalid block header: {}", e)))?;

            // Verify difficulty
            if !self.validate_difficulty(&header).await
                .map_err(|e| Status::internal(format!("Difficulty validation failed: {}", e)))?
            {
                return Ok(Response::new(VerifySPVProofResponse {
                    is_valid: false,
                    error: "Invalid difficulty".to_string(),
                }));
            }

            // Verify header chain
            if let Some(prev) = prev_hash {
                if header.prev_blockhash != prev {
                    return Ok(Response::new(VerifySPVProofResponse {
                        is_valid: false,
                        error: "Invalid header chain".to_string(),
                    }));
                }
            }
            prev_hash = Some(header.hash());
        }

        // Verify merkle path
        let mut current_hash = txid;
        for i in (0..merkle_path.len()).step_by(32) {
            let sibling = &merkle_path[i..i + 32];
            let mut combined = [0u8; 64];
            if i % 64 == 0 {
                combined[..32].copy_from_slice(&current_hash[..]);
                combined[32..].copy_from_slice(sibling);
            } else {
                combined[..32].copy_from_slice(sibling);
                combined[32..].copy_from_slice(&current_hash[..]);
            }
            current_hash = Sha256d::double_sha256(&combined);
        }

        // TODO: Compare with block merkle root
        let is_valid = true; // Placeholder

        let reply = VerifySPVProofResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Merkle path verification failed".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn batch_generate_spv_proof(&self, request: Request<BatchGenerateSPVProofRequest>) -> Result<Response<BatchGenerateSPVProofResponse>, Status> {
        let req = request.into_inner();
        let mut results = vec![];

        for txid in req.txids {
            let request = GenerateSPVProofRequest { txid };
            let result = self.generate_spv_proof(Request::new(request))
                .await?
                .into_inner();
            results.push(result);
        }

        let reply = BatchGenerateSPVProofResponse { results };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50057".parse().unwrap();
    let validation_service = ValidationServiceImpl::new().await;

    println!("Validation service listening on {}", addr);

    Server::builder()
        .add_service(ValidationServer::new(validation_service))
        .serve(addr)
        .await?;

    Ok(())
}
