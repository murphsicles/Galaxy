use tonic::{transport::Server, Request, Response, Status};
use consensus::consensus_server::{Consensus, ConsensusServer};
use consensus::{
    ValidateBlockConsensusRequest, ValidateBlockConsensusResponse,
    ValidateTxConsensusRequest, ValidateTxConsensusResponse,
    BatchValidateTxConsensusRequest, BatchValidateTxConsensusResponse
};
use sv::block::Block;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use hex;

tonic::include_proto!("consensus");

#[derive(Debug, Default)]
struct ConsensusServiceImpl;

impl ConsensusServiceImpl {
    async fn validate_block_rules(&self, block: &Block) -> Result<bool, String> {
        // BSV-specific rules: block size <= 4GB
        let block_size = serialize(block).len() as u64;
        if block_size > 4_000_000_000 {
            return Err(format!("Block size {} exceeds 4GB limit", block_size));
        }

        // TODO: Validate merkle root, difficulty, timestamp
        Ok(true)
    }

    async fn validate_transaction_rules(&self, tx: &Transaction) -> Result<bool, String> {
        // BSV-specific rules: transaction size, script validity
        let tx_size = serialize(tx).len() as u64;
        if tx_size > 1_000_000_000 { // BSV allows large transactions, but set a reasonable limit
            return Err(format!("Transaction size {} exceeds limit", tx_size));
        }

        for output in &tx.outputs {
            if output.script_pubkey.is_op_return() && output.script_pubkey.len() > 100_000 {
                return Err("OP_RETURN data exceeds 100KB limit".to_string());
            }
            if !output.script_pubkey.is_standard() {
                return Err("Non-standard script".to_string());
            }
        }

        // TODO: Validate additional BSV rules (e.g., no SegWit)
        Ok(true)
    }
}

#[tonic::async_trait]
impl Consensus for ConsensusServiceImpl {
    async fn validate_block_consensus(&self, request: Request<ValidateBlockConsensusRequest>) -> Result<Response<ValidateBlockConsensusResponse>, Status> {
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        let is_valid = match self.validate_block_rules(&block).await {
            Ok(_) => true,
            Err(e) => return Ok(Response::new(ValidateBlockConsensusResponse {
                is_valid: false,
                error: e,
            })),
        };

        let reply = ValidateBlockConsensusResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Block consensus validation failed".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn validate_transaction_consensus(&self, request: Request<ValidateTxConsensusRequest>) -> Result<Response<ValidateTxConsensusResponse>, Status> {
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        let is_valid = match self.validate_transaction_rules(&tx).await {
            Ok(_) => true,
            Err(e) => return Ok(Response::new(ValidateTxConsensusResponse {
                is_valid: false,
                error: e,
            })),
        };

        let reply = ValidateTxConsensusResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Transaction consensus validation failed".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn batch_validate_transaction_consensus(&self, request: Request<BatchValidateTxConsensusRequest>) -> Result<Response<BatchValidateTxConsensusResponse>, Status> {
        let req = request.into_inner();
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxConsensusRequest { tx_hex };
            let result = self.validate_transaction_consensus(Request::new(validate_request))
                .await?
                .into_inner();
            results.push(result);
        }

        let reply = BatchValidateTxConsensusResponse { results };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50055".parse().unwrap(); // Different port for consensus_service
    let consensus_service = ConsensusServiceImpl::default();

    println!("Consensus service listening on {}", addr);

    Server::builder()
        .add_service(ConsensusServer::new(consensus_service))
        .serve(addr)
        .await?;

    Ok(())
}
