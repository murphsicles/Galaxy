use tonic::{transport::Server, Request, Response, Status};
use transaction::transaction_server::{Transaction, TransactionServer};
use transaction::{ValidateTxRequest, ValidateTxResponse, ProcessTxRequest, ProcessTxResponse};
use sv::transaction::Transaction;
use sv::script::Script;
use hex;

tonic::include_proto!("transaction");

#[derive(Debug, Default)]
struct TransactionServiceImpl;

#[tonic::async_trait]
impl Transaction for TransactionServiceImpl {
    async fn validate_transaction(&self, request: Request<ValidateTxRequest>) -> Result<Response<ValidateTxResponse>, Status> {
        let req = request.into_inner();
        // Parse hex-encoded transaction
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = sv::util::deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        // Basic validation: check format and signatures
        let is_valid = if tx.inputs.is_empty() || tx.outputs.is_empty() {
            false
        } else {
            // TODO: Implement full signature and script validation
            true // Placeholder
        };

        let reply = ValidateTxResponse {
            is_valid,
            error: if is_valid { "".to_string() } else { "Invalid transaction format".to_string() },
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

        // TODO: Process transaction (e.g., store or forward to network_service)
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
    let addr = "[::1]:50052".parse().unwrap(); // Different port from network_service
    let transaction_service = TransactionServiceImpl::default();

    println!("Transaction service listening on {}", addr);

    Server::builder()
        .add_service(TransactionServer::new(transaction_service))
        .serve(addr)
        .await?;

    Ok(())
}
