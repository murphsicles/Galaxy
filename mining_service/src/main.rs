use tonic::{transport::{Server, Channel}, Request, Response, Status};
use mining::mining_server::{Mining, MiningServer};
use mining::{
    GetMiningWorkRequest, GetMiningWorkResponse,
    SubmitMinedBlockRequest, SubmitMinedBlockResponse,
    BatchGetMiningWorkRequest, BatchGetMiningWorkResponse
};
use block::block_client::BlockClient;
use block::AssembleBlockRequest;
use network::network_client::NetworkClient;
use network::BroadcastBlockRequest;
use sv::block::Block;
use sv::util::{deserialize, serialize};
use hex;
use toml;

tonic::include_proto!("mining");
tonic::include_proto!("block");
tonic::include_proto!("network");

#[derive(Debug)]
struct MiningServiceImpl {
    block_client: BlockClient<Channel>,
    network_client: NetworkClient<Channel>,
}

impl MiningServiceImpl {
    async fn new() -> Self {
        let block_client = BlockClient::connect("http://[::1]:50054")
            .await
            .expect("Failed to connect to block_service");
        let network_client = NetworkClient::connect("http://[::1]:50051")
            .await
            .expect("Failed to connect to network_service");
        MiningServiceImpl { block_client, network_client }
    }

    async fn generate_block_template(&self) -> Result<(String, u32), String> {
        // Load test transactions from config (for demo)
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let tx_hex = config["test_cases"]["broadcast_transaction"]["tx_hex"]
            .as_str()
            .unwrap()
            .to_string();

        // Assemble block via block_service
        let mut block_client = self.block_client.clone();
        let request = AssembleBlockRequest {
            tx_hexes: vec![tx_hex],
        };
        let response = block_client.assemble_block(request)
            .await
            .map_err(|e| format!("Block assembly failed: {}", e))?
            .into_inner();

        if !response.error.is_empty() {
            return Err(response.error);
        }

        // TODO: Compute realistic difficulty target
        let target_difficulty = 0x1d00ffff; // Placeholder difficulty
        Ok((response.block_hex, target_difficulty))
    }
}

#[tonic::async_trait]
impl Mining for MiningServiceImpl {
    async fn get_mining_work(&self, request: Request<GetMiningWorkRequest>) -> Result<Response<GetMiningWorkResponse>, Status> {
        let _req = request.into_inner();
        let (block_template, target_difficulty) = self.generate_block_template()
            .await
            .map_err(|e| Status::internal(format!("Failed to generate work: {}", e)))?;

        let reply = GetMiningWorkResponse {
            block_template,
            target_difficulty,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn submit_mined_block(&self, request: Request<SubmitMinedBlockRequest>) -> Result<Response<SubmitMinedBlockResponse>, Status> {
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        // TODO: Verify proof-of-work
        let is_valid_pow = true; // Placeholder

        if !is_valid_pow {
            return Ok(Response::new(SubmitMinedBlockResponse {
                success: false,
                error: "Invalid proof-of-work".to_string(),
            }));
        }

        // Broadcast block via network_service
        let mut network_client = self.network_client.clone();
        let broadcast_request = BroadcastBlockRequest { block_hex: req.block_hex };
        let broadcast_response = network_client.broadcast_block(broadcast_request)
            .await
            .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?
            .into_inner();

        if !broadcast_response.success {
            return Ok(Response::new(SubmitMinedBlockResponse {
                success: false,
                error: broadcast_response.error,
            }));
        }

        let reply = SubmitMinedBlockResponse {
            success: true,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn batch_get_mining_work(&self, request: Request<BatchGetMiningWorkRequest>) -> Result<Response<BatchGetMiningWorkResponse>, Status> {
        let req = request.into_inner();
        let mut results = vec![];

        for _miner_id in req.miner_ids {
            let result = self.get_mining_work(Request::new(GetMiningWorkRequest { miner_id: "".to_string() }))
                .await?
                .into_inner();
            results.push(result);
        }

        let reply = BatchGetMiningWorkResponse { results };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50058".parse().unwrap(); // Different port for mining_service
    let mining_service = MiningServiceImpl::new().await;

    println!("Mining service listening on {}", addr);

    Server::builder()
        .add_service(MiningServer::new(mining_service))
        .serve(addr)
        .await?;

    Ok(())
}
