use tonic::{transport::Server, Request, Response, Status};
use network::network_server::{Network, NetworkServer};
use network::{PingRequest, PingResponse};

tonic::include_proto!("network");

#[derive(Debug, Default)]
struct NetworkServiceImpl;

#[tonic::async_trait]
impl Network for NetworkServiceImpl {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let req = request.into_inner();
        let reply = PingResponse {
            reply: format!("Pong: {}", req.message),
        };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse().unwrap();
    let network_service = NetworkServiceImpl::default();

    println!("Network service listening on {}", addr);

    Server::builder()
        .add_service(NetworkServer::new(network_service))
        .serve(addr)
        .await?;

    Ok(())
}
