use tonic::{transport::{Server, Channel}, Request, Response, Status};
use block::block_server::{Block, BlockServer};
use block::{ValidateBlockRequest, ValidateBlockResponse, AssembleBlockRequest, AssembleBlockResponse};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use storage::storage_client::StorageClient;
use storage::{AddUtxoRequest, RemoveUtxoRequest};
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateBlockConsensusRequest;
use sv::block::Block;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use hex;

tonic::include_proto!("block");
tonic::include_proto!("transaction");
tonic::include_proto!("storage");
tonic::include_proto!("consensus");

#[derive(Debug)]
struct BlockServiceImpl {
    transaction_client: TransactionClient<Channel>,
    storage_client: StorageClient<Channel>,
    consensus_client: ConsensusClient<Channel>,
}

impl BlockServiceImpl {
    async fn new() -> Self {
        let transaction_client = TransactionClient::connect("http://[::1]:50052")
            .await
            .expect("Failed to connect to transaction_service");
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        let consensus_client = ConsensusClient::connect("http://[::1]:50055")
            .await
            .expect("Failed to connect to consensus_service");
        BlockServiceImpl { transaction_client, storage_client, consensus_client }
    }

    async fn validate_block_transactions(&self, block: &Block) -> Result<bool, String> {
        let mut client = self.transaction_client.clone();
        for tx in &block.transactions {
            let tx_hex = hex::encode(serialize(tx));
            let request = ValidateTxRequest { tx_hex };
            let response = client.validate_transaction(request)
                .await
                .map_err(|e| format!("Transaction validation failed: {}", e))?
                .into_inner();
            if !response.is_valid {
                return Err(response.error);
            }
        }
        Ok(true)
    }

    async fn update_utxos(&self, block: &Block) -> Result<(), String> {
        let mut storage_client = self.storage_client.clone();
        for tx in &block.transactions {
            for input in &tx.inputs {
                let request = RemoveUtxoRequest {
                    txid: input.previous_output.txid.to_string(),
                    vout: input.previous_output.vout,
                };
                let response = storage_client.remove_utxo(request)
                    .await
                    .map_err(|e| format!("Remove UTXO failed: {}", e))?
                    .into_inner();
                if !response.success {
                    return Err(response.error);
                }
            }
            for (vout, output) in tx.outputs.iter().enumerate() {
                let request = AddUtxoRequest {
                    txid: tx.txid().to_string(),
                    vout: vout as u32,
                    script_pubkey: hex::encode(&output.script_pubkey),
                    amount: output.value,
                };
                let response = storage_client.add_utxo(request)
                    .await
                    .map_err(|e| format!("Add UTXO failed: {}", e))?
                    .into_inner();
                if !response.success {
                    return Err(response.error);
                }
            }
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl Block for BlockServiceImpl {
    async fn validate_block(&self, request: Request<ValidateBlockRequest>) -> Result<Response<ValidateBlockResponse>, Status> {
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        // Validate block header (simplified)
        if block.header.version < 1 {
            return Ok(Response::new(ValidateBlockResponse {
                is_valid: false,
                error: "Invalid block header version".to_string(),
            }));
        }

        // Validate consensus rules
        let mut consensus_client = self.consensus_client.clone();
        let consensus_request = ValidateBlockConsensusRequest { block_hex: req.block_hex.clone() };
        let consensus_response = consensus_client.validate_block_consensus(consensus_request)
            .await
            .map_err(|e| Status::internal(format!("Consensus validation failed: {}", e)))?
            .into_inner();
        if !consensus_response.is_valid {
            return Ok(Response::new(ValidateBlockResponse {
                is_valid: false,
                error: consensus_response.error,
            }));
        }

        // Validate transactions
        let is_valid = match self.validate_block_transactions(&block).await {
            Ok(_) => true,
            Err(e) => return Ok(Response::new(ValidateBlockResponse {
                is_valid: false,
                error: e,
            })),
        };

        // Update UTXOs if valid
        if is_valid {
            if let Err(e) = self.update_utxos(&block).await {
                return Ok(Response::new(ValidateBlockResponse {
                    is_valid: false,
                    error: format!("UTXO update failed: {}", e),
                }));
            }
        }

        let reply = ValidateBlockResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Block validation failed".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn assemble_block(&self, request: Request<AssembleBlockRequest>) -> Result<Response<AssembleBlockResponse>, Status> {
        let req = request.into_inner();
        let mut transactions = vec![];

        let mut client = self.transaction_client.clone();
        for tx_hex in req.tx_hexes {
            let request = ValidateTxRequest { tx_hex: tx_hex.clone() };
            let response = client.validate_transaction(request)
                .await
                .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
                .into_inner();
            if !response.is_valid {
                return Ok(Response::new(AssembleBlockResponse {
                    block_hex: "".to_string(),
                    error: response.error,
                }));
            }
            let tx_bytes = hex::decode(&tx_hex)
                .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
            let tx: Transaction = deserialize(&tx_bytes)
                .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;
            transactions.push(tx);
        }

        let block = Block {
            header: sv::block::BlockHeader {
                version: 1,
                prev_blockhash: Default::default(),
                merkle_root: Default::default(), // TODO: Compute merkle root
                time: 0,
                bits: 0,
                nonce: 0,
            },
            transactions,
        };

        let block_hex = hex::encode(serialize(&block));
        let reply = AssembleBlockResponse {
            block_hex,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50054".parse().unwrap();
    let block_service = BlockServiceImpl::new().await;

    println!("Block service listening on {}", addr);

    Server::builder()
        .add_service(BlockServer::new(block_service))
        .serve(addr)
        .await?;

    Ok(())
}
