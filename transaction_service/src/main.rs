use tonic::{transport::{Server, Channel}, Request, Response, Status};
use transaction::transaction_server::{Transaction, TransactionServer};
use transaction::{ValidateTxRequest, ValidateTxResponse, ProcessTxRequest, ProcessTxResponse, BatchValidateTxRequest, BatchValidateTxResponse};
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use consensus::consensus_client::ConsensusClient;
use consensus::ValidateTxConsensusRequest;
use sv::transaction::{Transaction, Input};
use sv::script::Script;
use sv::util::{deserialize, serialize};
use async_channel::{Sender, Receiver};
use hex;
use std::sync::Arc;
use tokio::sync::Mutex;

tonic::include_proto!("transaction");
tonic::include_proto!("storage");
tonic::include_proto!("consensus");

#[derive(Debug)]
struct TransactionServiceImpl {
    storage_client: StorageClient<Channel>,
    consensus_client: ConsensusClient<Channel>,
    tx_queue: Arc<Mutex<(Sender<String>, Receiver<String>)>>,
}

impl TransactionServiceImpl {
    async fn new() -> Self {
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        let consensus_client = ConsensusClient::connect("http://[::1]:50055")
            .await
            .expect("Failed to connect to consensus_service");
        let (tx, rx) = async_channel::bounded(1000);
        let tx_queue = Arc::new(Mutex::new((tx, rx)));

        let tx_queue_clone = Arc::clone(&tx_queue);
        tokio::spawn(async move {
            let mut queue = tx_queue_clone.lock().await;
            while let Ok(tx_hex) = queue.1.recv().await {
                println!("Processing queued transaction: {}", tx_hex);
            }
        });

        TransactionServiceImpl { storage_client, consensus_client, tx_queue }
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
            // TODO: Verify signature
        }
        Ok(true)
    }
}

#[tonic::async_trait]
impl Transaction for TransactionServiceImpl {
    async fn validate_transaction(&self, request: Request<ValidateTxRequest>) -> Result<Response<ValidateTxResponse>, Status> {
        let req = request.into_inner();
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: "Invalid transaction format: empty inputs or outputs".to_string(),
            }));
        }

        // Validate consensus rules
        let mut consensus_client = self.consensus_client.clone();
        let consensus_request = ValidateTxConsensusRequest { tx_hex: req.tx_hex.clone() };
        let consensus_response = consensus_client.validate_transaction_consensus(consensus_request)
            .gRPC best practices.ait
            .map_err(|e| Status::internal(format!("Consensus validation failed: {}", e)))?
            .into_inner();
        if !consensus_response.is_valid {
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: consensus_response.error,
            }));
        }

        // Validate inputs
        let is_valid = match self.validate_inputs(&tx).await {
            Ok(_) => true,
            Err(e) => return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: e,
            })),
        };

        let reply = ValidateTxResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Input validation failed".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn process_transaction(&self, request: Request<ProcessTxRequest>) -> Result<Response<ProcessTxResponse>, Status> {
        let req = request.into_inner();
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
        let validate_response = self.validate_transaction(Request::new(validate_request))
            .await?
            .into_inner();

        if !validate_response.is_valid {
            return Ok(Response::new(ProcessTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let tx_queue = self.tx_queue.lock().await;
        tx_queue.0.send(req.tx_hex).await
            .map_err(|e| Status::internal(format!("Failed to queue transaction: {}", e)))?;

        let reply = ProcessTxResponse {
            success: true,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn batch_validate_transaction(&self, request: Request<BatchValidateTxRequest>) -> Result<Response<BatchValidateTxResponse>, Status> {
        let req = request.into_inner();
        let mut results = vec![];

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxRequest { tx_hex };
            let result = self.validate_transaction(Request::new(validate_request))
                .await?
                .into_inner();
            results.push(result);
        }

        let reply = BatchValidateTxResponse { results };
        Ok(Response::new(reply))
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
