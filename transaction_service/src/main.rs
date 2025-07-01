use tonic::{transport::{Server, Channel}, Request, Response, Status};
use transaction::transaction_server::{Transaction, TransactionServer};
use transaction::{ValidateTxRequest, ValidateTxResponse, ProcessTxRequest, ProcessTxResponse, BatchValidateTxRequest, BatchValidateTxResponse};
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateTxConsensusRequest;
use metrics::metrics_client::MetricsClient;
use sv::transaction::{Transaction, Input};
use sv::script::Script;
use sv::util::{deserialize, serialize};
use async_channel::{Sender, Receiver};
use hex;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Instant;
use prometheus::{Counter, Gauge, Registry};
use toml;

tonic::include_proto!("transaction");
tonic::include_proto!("storage");
tonic::include_proto!("consensus");
tonic::include_proto!("metrics");

#[derive(Debug)]
struct TransactionServiceImpl {
    storage_client: StorageClient<Channel>,
    consensus_client: ConsensusClient<Channel>,
    tx_queue: Arc<Mutex<(Sender<String>, Receiver<String>)>>,
    registry: Arc<Registry>,
    requests_total: Counter,
    latency_ms: Gauge,
}

impl TransactionServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let shard_id = config["sharding"]["shard_id"].as_integer().unwrap_or(0) as u32;

        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        let consensus_client = ConsensusClient::connect("http://[::1]:50055")
            .await
            .expect("Failed to connect to consensus_service");
        let (tx, rx) = async_channel::bounded(1000);
        let tx_queue = Arc::new(Mutex::new((tx, rx)));
        let registry = Arc::new(Registry::new());
        let requests_total = Counter::new("transaction_requests_total", "Total transaction requests").unwrap();
        let latency_ms = Gauge::new("transaction_latency_ms", "Average transaction processing latency").unwrap();
        registry.register(Box::new(requests_total.clone())).unwrap();
        registry.register(Box::new(latency_ms.clone())).unwrap();

        let tx_queue_clone = Arc::clone(&tx_queue);
        tokio::spawn(async move {
            let mut queue = tx_queue_clone.lock().await;
            while let Ok(tx_hex) = queue.1.recv().await {
                println!("Processing queued transaction: {}", tx_hex);
            }
        });

        TransactionServiceImpl { storage_client, consensus_client, tx_queue, registry, requests_total, latency_ms }
    }

    async fn validate_inputs(&self, tx: &Transaction) -> Result<bool, String> {
        let mut client = self.storage_client.clone();
        for input in &tx.inputs {
            let request = QueryUtxoRequest {
                txid: input.previous_output.txid.to_string(),
                vout: input.previous_output.vout,
            };
            let response = client.query_utxo(request)
                .await
                .map_err(|e| format!("UTXO query failed: {}", e))?
                .into_inner();

            if !response.exists {
                return Err(format!("UTXO not found: {}:{}", input.previous_output.txid, input.previous_output.vout));
            }

            let script_pubkey: Script = deserialize(&hex::decode(&response.script_pubkey)
                .map_err(|e| format!("Invalid script_pubkey: {}", e))?)?;
            if !script_pubkey.is_standard() {
                return Err("Non-standard script".to_string());
            }
        }
        Ok(true)
    }
}

#[tonic::async_trait]
impl Transaction for TransactionServiceImpl {
    async fn validate_transaction(&self, request: Request<ValidateTxRequest>) -> Result<Response<ValidateTxResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: "Invalid transaction format: empty inputs or outputs".to_string(),
            }));
        }

        let mut consensus_client = self.consensus_client.clone();
        let consensus_request = ValidateTxConsensusRequest { tx_hex: req.tx_hex.clone() };
        let consensus_response = consensus_client.validate_transaction_consensus(consensus_request)
            .await
            .map_err(|e| Status::internal(format!("Consensus validation failed: {}", e)))?
            .into_inner();
        if !consensus_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: consensus_response.error,
            }));
        }

        let is_valid = match self.validate_inputs(&tx).await {
            Ok(_) => true,
            Err(e) => {
                self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
                return Ok(Response::new(ValidateTxResponse {
                    is_valid: false,
                    error: e,
                }));
            }
        };

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ValidateTxResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Input validation failed".to_string() },
        }))
    }

    async fn process_transaction(&self, request: Request<ProcessTxRequest>) -> Result<Response<ProcessTxResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
        let validate_response = self.validate_transaction(Request::new(validate_request))
            .await?
            .into_inner();

        if !validate_response.is_valid {
            self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
            return Ok(Response::new(ProcessTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let tx_queue = self.tx_queue.lock().await;
        tx_queue.0.send(req.tx_hex).await
            .map_err(|e| Status::internal(format!("Failed to queue transaction: {}", e)))?;

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(ProcessTxResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn batch_validate_transaction(&self, request: Request<BatchValidateTxRequest>) -> Result<Response<BatchValidateTxResponse>, Status> {
        self.requests_total.inc();
        let start = Instant::now();
        let req = request.into_inner();
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxRequest { tx_hex };
            let result = self.validate_transaction(Request::new(validate_request))
                .await?
                .into_inner();
            results.push(result);
        }

        self.latency_ms.set(start.elapsed().as_secs_f64() * 1000.0);
        Ok(Response::new(BatchValidateTxResponse { results }))
    }

    async fn get_metrics(&self, _request: Request<GetMetricsRequest>) -> Result<Response<GetMetricsResponse>, Status> {
        Ok(Response::new(GetMetricsResponse {
            service_name: "transaction_service".to_string(),
            requests_total: self.requests_total.get() as u64,
            avg_latency_ms: self.latency_ms.get(),
            errors_total: 0, // Placeholder
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50052".parse().unwrap();
    let transaction_service = TransactionServiceImpl::new().await;

    println!("Transaction service listening on {}", addr);

    Server::builder()
        .add_service(TransactionServer::new(transaction_service))
        .serve(addr)
        .await?;

    Ok(())
}
