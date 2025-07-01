use tonic::{transport::Server, Request, Response, Status};
use consensus::consensus_server::{Consensus, ConsensusServer};
use consensus::{
    ValidateBlockConsensusRequest, ValidateBlockConsensusResponse,
    ValidateTxConsensusRequest, ValidateTxConsensusResponse,
    BatchValidateTxConsensusRequest, BatchValidateTxConsensusResponse
};
use metrics::metrics_client::MetricsClient;
use metrics::{GetMetricsRequest, GetMetricsResponse};
use sv::block::Block;
use sv::transaction::Transaction;
use sv::util::{deserialize, serialize};
use hex;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use toml;

tonic::include_proto!("consensus");
tonic::include_proto!("metrics");

#[derive(Debug, Default)]
struct ConsensusServiceImpl {
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
}

impl ConsensusServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("consensus_requests_total", "Total consensus requests").unwrap();
        let latency_ms = Gauge::new("consensus_latency_ms", "Average consensus request latency").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();

        ConsensusServiceImpl {
            registry,
            requests_total,
            latency_ms,
        }
    }

    async fn validate_block_rules(&self, block: &Block) -> Result<bool, String> {
        let block_size = serialize(block).len() as u64;
        if block_size > 4_000_000_000 {
            return Err(format!("Block size {} exceeds 4GB limit", block_size));
        }

        // TODO: Validate merkle root, difficulty, timestamp
        Ok(true)
    }

    async fn validate_transaction_rules(&self, tx: &Transaction) -> Result<bool, String> {
        let tx_size = serialize(tx).len() as u64;
        if tx_size > 1_000_000_000 {
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

        Ok(true)
    }
}

#[tonic::async_trait]
impl Consensus for ConsensusServiceImpl {
    async fn validate_block_consensus(&self, request: Request<ValidateBlockConsensusRequest>) -> Result<Response<ValidateBlockConsensusResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        let is_valid = match self.validate_block_rules(&block).await {
            Ok(_) => true,
            Err(e) => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                return Ok(Response::new(ValidateBlockConsensusResponse {
                    is_valid: false,
                    error: e,
                }));
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateBlockConsensusResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Block consensus validation failed".to_string() },
        }))
    }

    async fn validate_transaction_consensus(&self, request: Request<ValidateTxConsensusRequest>) -> Result<Response<ValidateTxConsensusResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        let is_valid = match self.validate_transaction_rules(&tx).await {
            Ok(_) => true,
            Err(e) => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                return Ok(Response::new(ValidateTxConsensusResponse {
                    is_valid: false,
                    error: e,
                }));
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateTxConsensusResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Transaction consensus validation failed".to_string() },
        }))
    }

    async fn batch_validate_transaction_consensus(&self, request: Request<BatchValidateTxConsensusRequest>) -> Result<Response<BatchValidateTxConsensusResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxConsensusRequest { tx_hex };
            let result = self
                .validate_transaction_consensus(Request::new(validate_request))
                .await?
                .into_inner();
            results.push(result);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchValidateTxConsensusResponse { results }))
    }

    async fn get_metrics(&self, _request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        Ok(Response::new(GetMetricsResponse {
            service_name: "consensus_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50055".parse().unwrap();
    let consensus_service = ConsensusServiceImpl::new().await;

    println!("Consensus service listening on {}", addr);

    Server::builder()
        .add_service(ConsensusServer::new(consensus_service))
        .serve(addr)
        .await?;

    Ok(())
}
