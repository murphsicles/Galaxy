use tonic::{transport::Server, Request, Response, Status};
use network::network_server::{Network, NetworkServer};
use network::{
    PingRequest, PingResponse, DiscoverPeersRequest, DiscoverPeersResponse,
    BroadcastTxRequest, BroadcastTxResponse, BroadcastBlockRequest, BroadcastBlockResponse
};
use sv::messages::{Message, NetworkMessage};
use sv::network::{Network as BSVNetwork, Version};
use sv::transaction::Transaction;
use sv::block::Block;
use tokio::net::TcpStream;
use hex;

tonic::include_proto!("network");

#[derive(Debug)]
struct NetworkServiceImpl {
    peers: Vec<String>,
}

impl NetworkServiceImpl {
    fn new() -> Self {
        // Initialize with known BSV testnet nodes
        let peers = vec![
            "testnet-seed.bitcoin.sipa.be:18333".to_string(),
            "testnet-seed.bsv.io:18333".to_string(),
        ];
        NetworkServiceImpl { peers }
    }

    async fn send_version_message(&self, addr: &str) -> Result<(), String> {
        // Connect to a peer
        let mut stream = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;

        // Create a BSV Version message
        let version = Version {
            version: 70015,
            services: 0,
            timestamp: 0,
            receiver_services: 0,
            receiver_ip: [0; 16],
            receiver_port: 0,
            sender_services: 0,
            sender_ip: [0; 16],
            sender_port: 0,
            nonce: 0,
            user_agent: "/Galaxy:0.1.0/".to_string(),
            start_height: 0,
            relay: false,
        };
        let message = Message {
            command: "version".to_string(),
            payload: version.serialize(),
        };

        // Send the Version message
        let serialized = message.serialize(BSVNetwork::Testnet);
        stream.write_all(&serialized).await.map_err(|e| e.to_string())?;

        // Wait for Verack (simplified)
        let mut buffer = vec![0u8; 1024];
        let n = stream.read(&mut buffer).await.map_err(|e| e.to_string())?;
        let response = Message::deserialize(&buffer[..n], BSVNetwork::Testnet)
            .map_err(|e| e.to_string())?;
        if response.command == "verack" {
            Ok(())
        } else {
            Err("Expected Verack".to_string())
        }
    }
}

#[tonic::async_trait]
impl Network for NetworkServiceImpl {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let req = request.into_inner();
        let reply = PingResponse {
            reply: format!("Pong: {}", req.message),
        };
        Ok(Response::new(reply))
    }

    async fn discover_peers(&self, _request: Request<DiscoverPeersRequest>) -> Result<Response<DiscoverPeersResponse>, Status> {
        let peer = self.peers.get(0).ok_or_else(|| Status::internal("No peers available"))?;
        match self.send_version_message(peer).await {
            Ok(_) => {
                let reply = DiscoverPeersResponse {
                    peer_addresses: self.peers.clone(),
                    error: "".to_string(),
                };
                Ok(Response::new(reply))
            }
            Err(e) => {
                let reply = DiscoverPeersResponse {
                    peer_addresses: vec![],
                    error: e,
                };
                Ok(Response::new(reply))
            }
        }
    }

    async fn broadcast_transaction(&self, request: Request<BroadcastTxRequest>) -> Result<Response<BroadcastTxResponse>, Status> {
        let req = request.into_inner();
        // Parse hex-encoded transaction
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = sv::util::deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        // Simulate broadcasting to peers (to be expanded)
        for peer in &self.peers {
            println!("Broadcasting tx {} to {}", tx.txid(), peer);
        }

        let reply = BroadcastTxResponse {
            success: true,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn broadcast_block(&self, request: Request<BroadcastBlockRequest>) -> Result<Response<BroadcastBlockResponse>, Status> {
        let req = request.into_inner();
        // Parse hex-encoded block
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = sv::util::deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        // Simulate broadcasting to peers (to be expanded)
        for peer in &self.peers {
            println!("Broadcasting block {} to {}", block.block_hash(), peer);
        }

        let reply = BroadcastBlockResponse {
            success: true,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse().unwrap();
    let network_service = NetworkServiceImpl::new();

    println!("Network service listening on {}", addr);

    Server::builder()
        .add_service(NetworkServer::new(network_service))
        .serve(addr)
        .await?;

    Ok(())
}
