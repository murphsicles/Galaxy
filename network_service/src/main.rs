use tonic::{transport::{Server, Channel}, Request, Response, Status};
use network::network_server::{Network, NetworkServer};
use network::{
    PingRequest, PingResponse, DiscoverPeersRequest, DiscoverPeersResponse,
    BroadcastTxRequest, BroadcastTxResponse, BroadcastBlockRequest, BroadcastBlockResponse
};
use transaction::transaction_client::TransactionClient;
use transaction::ValidateTxRequest;
use sv::messages::{Message, NetworkMessage};
use sv::network::{Network as BSVNetwork, Version};
use sv::transaction::Transaction;
use sv::block::Block;
use tokio::net::{TcpStream, TcpListener};
use tokio::sync::mpsc;
use futures::stream::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use hex;

tonic::include_proto!("network");
tonic::include_proto!("transaction");

#[derive(Debug)]
struct NetworkServiceImpl {
    peers: Arc<Mutex<HashMap<String, mpsc::Sender<Message>>>>,
    transaction_client: TransactionClient<Channel>,
}

impl NetworkServiceImpl {
    async fn new() -> Self {
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let initial_peers = vec![
            "testnet-seed.bitcoin.sipa.be:18333".to_string(),
            "testnet-seed.bsv.io:18333".to_string(),
        ];

        // Connect to transaction_service
        let transaction_client = TransactionClient::connect("http://[::1]:50052")
            .await
            .expect("Failed to connect to transaction_service");

        // Spawn a task to manage peer connections
        let peers_clone = Arc::clone(&peers);
        tokio::spawn(async move {
            for peer in initial_peers {
                if let Ok(stream) = TcpStream::connect(&peer).await {
                    let (tx, mut rx) = mpsc::channel(100);
                    {
                        let mut peers = peers_clone.lock().await;
                        peers.insert(peer.clone(), tx);
                    }
                    tokio::spawn(Self::handle_peer(stream, peer, peers_clone.clone()));
                }
            }
        });

        NetworkServiceImpl { peers, transaction_client }
    }

    async fn send_version_message(stream: &mut TcpStream, addr: &str) -> Result<(), String> {
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
        let serialized = message.serialize(BSVNetwork::Testnet);
        stream.write_all(&serialized).await.map_err(|e| e.to_string())?;

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

    async fn handle_peer(stream: TcpStream, addr: String, peers: Arc<Mutex<HashMap<String, mpsc::Sender<Message>>>>) {
        let mut stream = stream;
        if Self::send_version_message(&mut stream, &addr).await.is_err() {
            let mut peers = peers.lock().await;
            peers.remove(&addr);
            return;
        }

        let mut buffer = vec![0u8; 1024];
        loop {
            match stream.read(&mut buffer).await {
                Ok(n) if n > 0 => {
                    if let Ok(message) = Message::deserialize(&buffer[..n], BSVNetwork::Testnet) {
                        match message.command.as_str() {
                            "addr" => {
                                println!("Received addr message from {}", addr);
                            }
                            "inv" => {
                                println!("Received inv message from {}", addr);
                            }
                            "getdata" => {
                                println!("Received getdata message from {}", addr);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {
                    let mut peers = peers.lock().await;
                    peers.remove(&addr);
                    break;
                }
            }
        }
    }

    async fn broadcast_message(&self, message: Message) -> Result<(), String> {
        let peers = self.peers.lock().await;
        for (addr, tx) in peers.iter() {
            if let Err(e) = tx.send(message.clone()).await {
                println!("Failed to send message to {}: {}", addr, e);
            }
        }
        Ok(())
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
        let peers = self.peers.lock().await;
        let peer_addresses: Vec<String> = peers.keys().cloned().collect();
        let reply = DiscoverPeersResponse {
            peer_addresses,
            error: if peers.is_empty() { "No active peers".to_string() } else { "".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn broadcast_transaction(&self, request: Request<BroadcastTxRequest>) -> Result<Response<BroadcastTxResponse>, Status> {
        let req = request.into_inner();
        // Validate transaction via transaction_service
        let mut client = self.transaction_client.clone();
        let validate_request = ValidateTxRequest {
            tx_hex: req.tx_hex.clone(),
        };
        let validate_response = client.validate_transaction(validate_request)
            .await
            .map_err(|e| Status::internal(format!("Validation failed: {}", e)))?
            .into_inner();

        if !validate_response.is_valid {
            return Ok(Response::new(BroadcastTxResponse {
                success: false,
                error: validate_response.error,
            }));
        }

        // Parse and broadcast transaction
        let tx_bytes = hex::decode(&req.tx_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid tx_hex: {}", e)))?;
        let tx: Transaction = sv::util::deserialize(&tx_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid transaction: {}", e)))?;

        let message = Message {
            command: "tx".to_string(),
            payload: tx.serialize(),
        };
        self.broadcast_message(message).await
            .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?;

        let reply = BroadcastTxResponse {
            success: true,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn broadcast_block(&self, request: Request<BroadcastBlockRequest>) -> Result<Response<BroadcastBlockResponse>, Status> {
        let req = request.into_inner();
        let block_bytes = hex::decode(&req.block_hex)
            .map_err(|e| Status::invalid_argument(format!("Invalid block_hex: {}", e)))?;
        let block: Block = sv::util::deserialize(&block_bytes)
            .map_err(|e| Status::invalid_argument(format!("Invalid block: {}", e)))?;

        let message = Message {
            command: "block".to_string(),
            payload: block.serialize(),
        };
        self.broadcast_message(message).await
            .map_err(|e| Status::internal(format!("Broadcast failed: {}", e)))?;

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
    let network_service = NetworkServiceImpl::new().await;

    println!("Network service listening on {}", addr);

    Server::builder()
        .add_service(NetworkServer::new(network_service))
        .serve(addr)
        .await?;

    Ok(())
}
