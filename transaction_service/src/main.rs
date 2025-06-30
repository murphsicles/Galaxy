use tonic::{transport::{Server, Channel}, Request, Response, Status};
use transaction::transaction_server::{Transaction, TransactionServer};
use transaction::{ValidateTxRequest, ValidateTxResponse, ProcessTxRequest, ProcessTxResponse};
use storage::storage_client::StorageClient;
use storage::QueryUtxoRequest;
use sv::transaction::Transaction;
use sv::script::Script;
use hex;

tonic::include_proto!("transaction");
tonic::include_proto!("storage");

#[derive(Debug)]
struct TransactionServiceImpl {
    storage_client: StorageClient<Channel>,
}

impl TransactionServiceImpl {
    async fn new() -> Self {
        // Connect to storage_service
        let storage_client = StorageClient::connect("http://[::1]:50053")
            .await
            .expect("Failed to connect to storage_service");
        TransactionServiceImpl { storage_client }
    }

    async fn validate_inputs(&self, tx: &Transaction) -> Result<bool, String> {
        let mut client = self.storage_client.clone();
        for input in &tx.inputs {
            // Query UTXO for each input
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

            // TODO: Verify scriptPubKey and signature
            // Placeholder: Assume valid if UTXO exists
        }
        Ok(true)
    }
}

#[tonic::async_trait]
impl Transaction for TransactionServiceImpl {
    async fn validate_transaction(&self, request: Request<ValidateTxRequest>) -> Result<Response<ValidateTxResponse>, Status> {
        let req = request.into_inner();
        // Parse hex-encoded transaction
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = sv::util::deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        // Basic validation: check format
        if tx.inputs.is_empty() || tx.outputs.is_empty() {
            return Ok(Response::new(ValidateTxResponse {
                is_valid: false,
                error: "Invalid transaction format: empty inputs or outputs".to_string(),
            }));
        }

        // Validate inputs against UTXO set
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
        // Parse hex-encoded transaction
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = sv::util::deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        // Validate before processing
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex };
        let validate_response = self.validate_transaction(Request::new(validate_request))
            .await?
            .into_inner();

        if !validate_response.is_valid {
            return Ok(Response::new(ProcessTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        // TODO: Process transaction (e.g., forward to network_service or store)
        println!("Processing transaction {}", tx.txid());

        let reply = ProcessTxResponse {
            success: true,
            error: "".to_string(),
        };
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
