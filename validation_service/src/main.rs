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
        // TODO: Implement merkle path calculation
        // Placeholder: Return a dummy merkle path
        Ok(hex::decode("0000000000000000000000000000000000000000000000000000000000000000")
            .map_err(|e| format!("Failed to encode merkle path: {}", e))?)
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
                merkle_root: Default::default(),
                time: 0,
                bits: 0,
                nonce: 0,
            },
            transactions: vec![],
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

        // Verify block headers
        let mut is_valid = true;
        for header_hex in req.block_headers {
            let header_bytes = hex::decode(&header_hex)
                .map_err(|e| Status::invalid_argument(format!("Invalid block header: {}", e)))?;
            let header: sv::block::BlockHeader = deserialize(&header_bytes)
                .map_err(|e| Status::invalid_argument(format!("Invalid block header: {}", e)))?;
            // TODO: Verify header chain and difficulty
            if header.version < 1 {
                is_valid = false;
                break;
            }
        }

        // TODO: Verify merkle path
        // Placeholder: Assume valid if headers are valid
        let reply = VerifySPVProofResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Invalid block headers".to_string() },
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
    let addr = "[::1]:50057".parse().unwrap(); // Different port for validation_service
    let validation_service = ValidationServiceImpl::new().await;

    println!("Validation service listening on {}", addr);

    Server::builder()
        .add_service(ValidationServer::new(validation_service))
        .serve(addr)
        .await?;

    Ok(())
}
