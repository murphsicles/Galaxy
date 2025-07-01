use tonic::{transport::{Server, Channel}, Request, Response, Status};
use block::block_server::{Block, BlockServer};
use block::{ValidateBlockRequest, ValidateBlockResponse, AssembleBlockRequest, AssembleBlockResponse};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use storage::storage_client::StorageClient;
use storage::{AddUtxoRequest, RemoveUtxoRequest};
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateBlockConsensusRequest;
use metrics::metrics_client::MetricsClient;
use sv::block::Block;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize, hash::Sha256d};
use hex;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use toml;

tonic::include_proto!("block");
tonic::include_proto!("transaction");
tonic::include_proto!("storage");
tonic::include_proto!("consensus");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct BlockServiceImpl {
    transaction_client: TransactionClient<Channel>,
    storage_client: StorageClient<Channel>,
    consensus_client: ConsensusClient<Channel>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
}

impl BlockServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let transaction_client = TransactionClient::connect("http://[::1]:50052")
            .await
            .expect("Failed to connect to transaction_service");
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        let consensus_client = ConsensusClient::connect("http://[::1]:50055")
            .await
            .expect("Failed to connect to consensus_service");
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("block_requests_total", "Total block requests").unwrap();
        let latency_ms = Gauge::new("block_latency_ms", "Average block processing latency").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();

        BlockServiceImpl {
            transaction_client,
            storage_client,
            consensus_client,
            registry,
            requests_total,
            latency_ms,
        }
    }

    async fn validate_block_transactions(&self, block: &Block) -> Result<bool, String> {
        let mut client = self.transaction_client.clone();
        for tx in &block.transactions {
            let tx_hex = hex::encode(serialize(tx));
            let request = ValidateTxRequest { tx_hex };
            let response = client
                .validate_transaction(request)
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
                let response = storage_client
                    .remove_utxo(request)
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
                let response = storage_client
                    .add_utxo(request)
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
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        if block.header.version < 1 {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(ValidateBlockResponse {
                is_valid: false,
                error: "Invalid block header version".to_string(),
            }));
        }

        let mut consensus_client = self.consensus_client.clone();
        let consensus_request = ValidateBlockConsensusRequest {
            block_hex: req.block_hex.clone(),
        };
        let consensus_response = consensus_client
            .validate_block_consensus(consensus_request)
            .await
            .map_err(|e| Status::internal(format!("Consensus validation failed: {}", e)))?
            .into_inner();
        if !consensus_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(ValidateBlockResponse {
                is_valid: false,
                error: consensus_response.error,
            }));
        }

        let is_valid = match self.validate_block_transactions(&block).await {
            Ok(_) => true,
            Err(e) => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                return Ok(Response::new(ValidateBlockResponse {
                    is_valid: false,
                    error: e,
                }));
            }
        };

        if is_valid {
            if let Err(e) = self.update_utxos(&block).await {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                return Ok(Response::new(ValidateBlockResponse {
                    is_valid: false,
                    error: format!("UTXO update failed: {}", e),
                }));
            }
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateBlockResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Block validation failed".to_string() },
        }))
    }

    async fn assemble_block(&self, request: Request<AssembleBlockRequest>) -> Result<Response<AssembleBlockResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let mut transactions = vec![];

        let mut client = self.transaction_client.clone();
        for tx_hex in req.tx_hexes {
            let request = ValidateTxRequest { tx_hex: tx_hex.clone() };
            let response = client
                .validate_transaction(request)
                .await
                .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
                .into_inner();
            if !response.is_valid {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
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

        let txids: Vec<Sha256d> = transactions.iter().map(|tx| tx.txid()).collect();
        let merkle_root = if txids.is_empty() {
            Sha256d::from_hex("0000000000000000000000000000000000000000000000000000000000000000").unwrap()
        } else {
            let mut current_level = txids;
            while current_level.len() > 1 {
                let mut next_level = vec![];
                for i in (0..current_level.len()).step_by(2) {
                    let left = current_level[i];
                    let right = if i + 1 < current_level.len() { current_level[i + 1] } else { left };
                    let mut combined = [0u8; 64];
                    combined[..32].copy_from_slice(&left[..]);
                    combined[32..].copy_from_slice(&right[..]);
                    let parent = Sha256d::double_sha256(&combined);
                    next_level.push(parent);
                }
                current_level = next_level;
            }
            current_level[0]
        };

        let block = Block {
            header: sv::block::BlockHeader {
                version: 1,
                prev_blockhash: Default::default(),
                merkle_root,
                time: 0,
                bits: 0x1d00ffff, // Placeholder
                nonce: 0,
            },
            transactions,
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(AssembleBlockResponse {
            block_hex: hex::encode(serialize(&block)),
            error: "".to_string(),
        }))
    }

    async fn get_metrics(&self, _request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        Ok(Response::new(GetMetricsResponse {
            service_name: "block_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
        }))
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
