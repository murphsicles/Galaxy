use merchant_service::service::MerchantService;
use std::net::SocketAddr;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let service = MerchantService::new().await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 50063));
    service.run(&addr.to_string()).await
}
