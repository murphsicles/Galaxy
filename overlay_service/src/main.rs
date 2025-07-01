use tonic::{transport::{Server, Channel}, Request, Response, Status};
use overlay::overlay_server::{Overlay, OverlayServer};
use overlay::{
    CreateOverlayRequest, CreateOverlayResponse, SubmitOverlayTxRequest, SubmitOverlayTxResponse,
    GetOverlayBlockRequest, GetOverlayBlockResponse, BatchSubmitOverlayTxRequest, BatchSubmitOverlayTxResponse
};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use network::network_client::NetworkClient;
use network::BroadcastTxRequest;
use sv::transaction::Transaction;
use sv::block::Block;
use sv::util::{deserialize, serialize};
use hex;
use std::collections::HashMap;
use tokio::sync::Mutex;
use std::sync::Arc;

tonic::include_proto!("overlay");
tonic::include_proto!("transaction");
tonic::include_proto!("block");
tonic::include_proto!("network");

#[derive(Debug)]
struct OverlayServiceImpl {
    transaction_client: TransactionClient<Channel>,
    block_client: BlockClient<Channel>,
    network_client: NetworkClient<Channel>,
    overlays: Arc<Mutex<HashMap<String, (Vec<Block>, Vec<Transaction>)>>>, // Overlay ID to (blocks, pending txs)
}

impl OverlayServiceImpl {
    async fn new() -> Self {
        let transaction_client = TransactionClient::connect("http://[::1]:50052")
            .await
            .expect("Failed to connect to transaction_service");
        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let network_client = NetworkClient::connect("http://[::1]:50051")
            .await
            .expect("Failed to connect to network_service");
        let overlays = Arc::new(Mutex::new(HashMap::new()));
        OverlayServiceImpl { transaction_client, block_client, network_client, overlays }
    }

    async fn assemble_overlay_block(&self, overlay_id: &str, transactions: Vec<Transaction>) -> Result<Block, String> {
        let mut block_client = self.block_client.clone();
        let tx_hexes = transactions.iter()
            .map(|tx| hex::encode(serialize(tx)))
            .collect();
        let request = AssembleBlockRequest { tx_hexes };
        let response = block_client.assemble_block(request)
            .await
            .map_err(|e| format!("Block assembly failed: {}", e))?
            .into_inner();

        if !response.error.is_empty() {
            return Err(response.error);
        }

        let block: Block = deserialize(&hex::decode(&response.block_hex)
            .map_err(|e| format!("Invalid block_hex: {}", e))?)
            .map_err(|e| format!("Invalid block: {}", e))?;
        Ok(block)
    }
}

#[tonic::async_trait]
impl Overlay for OverlayServiceImpl {
    async fn create_overlay(&self, request: Request<CreateOverlayRequest>) -> Result<Response<CreateOverlayResponse>, Status> {
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        if overlays.contains_key(&req.overlay_id) {
            return Ok(Response::new(CreateOverlayResponse {
                success: false,
                error: "Overlay already exists".to_string(),
            }));
        }
        overlays.insert(req.overlay_id, (vec![], vec![]));
        Ok(Response::new(CreateOverlayResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn submit_overlay_transaction(&self, request: Request<SubmitOverlayTxRequest>) -> Result<Response<SubmitOverlayTxResponse>, Status> {
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let (blocks, pending_txs) = overlays.get_mut(&req.overlay_id)
            .ok_or_else(|| Status::not_found("Overlay not found"))?;

        let mut transaction_client = self.transaction_client.clone();
        let validate_request = ValidateTxRequest { tx_hex: req.tx_hex.clone() };
        let validate_response = transaction_client.validate_transaction(validate_request)
            .await
            .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
            .into_inner();
        if !validate_response.is_valid {
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        let mut network_client = self.network_client.clone();
        let broadcast_request = BroadcastTxRequest { tx_hex: req.tx_hex.clone() };
        let broadcast_response = network_client.broadcast_transaction(broadcast_request)
            .await
            .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?
            .into_inner();
        if !broadcast_response.success {
            return Ok(Response::new(SubmitOverlayTxResponse {
                success: false,
                error: broadcast_response.error,
            }));
        }

        pending_txs.push(tx);
        if pending_txs.len() >= 100 { // Arbitrary threshold for block assembly
            let block = self.assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                .await
                .map_err(|e| Status::internal(format!("Block assembly failed: {}", e)))?;
            blocks.push(block);
        }

        Ok(Response::new(SubmitOverlayTxResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn get_overlay_block(&self, request: Request<GetOverlayBlockRequest>) -> Result<Response<GetOverlayBlockResponse>, Status> {
        let req = request.into_inner();
        let overlays = self.overlays.lock().await;
        let (blocks, _) = overlays.get(&req.overlay_id)
            .ok_or_else(|| Status::not_found("Overlay not found"))?;

        if req.block_height as usize >= blocks.len() {
            return Ok(Response::new(GetOverlayBlockResponse {
                block_hex: "".to_string(),
                error: "Block height out of range".to_string(),
            }));
        }

        let block = &blocks[req.block_height as usize];
        let block_hex = hex::encode(serialize(block));
        Ok(Response::new(GetOverlayBlockResponse {
            block_hex,
            error: "".to_string(),
        }))
    }

    async fn batch_submit_overlay_transaction(&self, request: Request<BatchSubmitOverlayTxRequest>) -> Result<Response<BatchSubmitOverlayTxResponse>, Status> {
        let req = request.into_inner();
        let mut overlays = self.overlays.lock().await;
        let (blocks, pending_txs) = overlays.get_mut(&req.overlay_id)
            .ok_or_else(|| Status::not_found("Overlay not found"))?;

        let mut results = vec![];
        let mut transaction_client = self.transaction_client.clone();
        let mut network_client = self.network_client.clone();

        for tx_hex in req.tx_hexes {
            let validate_request = ValidateTxRequest { tx_hex: tx_hex.clone() };
            let validate_response = transaction_client.validate_transaction(validate_request)
                .await
                .map_err(|e| Status::internal(format!("Transaction validation failed: {}", e)))?
                .into_inner();
            if !validate_response.is_valid {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: validate_response.error,
                });
                continue;
            }

            let broadcast_request = BroadcastTxRequest { tx_hex: tx_hex.clone() };
            let broadcast_response = network_client.broadcast_transaction(broadcast_request)
                .await
                .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?
                .into_inner();
            if !broadcast_response.success {
                results.push(SubmitOverlayTxResponse {
                    success: false,
                    error: broadcast_response.error,
                });
                continue;
            }

            let tx_bytes = hex::decode(&tx_hex)
                .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
            let tx: Transaction = deserialize(&tx_bytes)
                .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;
            pending_txs.push(tx);

            results.push(SubmitOverlayTxResponse {
                success: true,
                error: "".to_string(),
            });
        }

        if pending_txs.len() >= 100 {
            let block = self.assemble_overlay_block(&req.overlay_id, pending_txs.drain(..).collect())
                .await
                .map_err(|e| Status::internal(format!("Block assembly failed: {}", e)))?;
            blocks.push(block);
        }

        Ok(Response::new(BatchSubmitOverlayTxResponse { results }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50056".parse().unwrap();
    let overlay_service = OverlayServiceImpl::new().await;

    println!("Overlay service listening on {}", addr);

    Server::builder()
        .add_service(OverlayServer::new(overlay_service))
        .serve(addr)
        .await?;

    Ok(())
}
