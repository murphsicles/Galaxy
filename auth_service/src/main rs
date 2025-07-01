use tonic::{transport::Server, Request, Response, Status};
use auth::auth_server::{Auth, AuthServer};
use auth::{AuthenticateRequest, AuthenticateResponse, AuthorizeRequest, AuthorizeResponse};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use toml;

tonic::include_proto!("auth");

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // User ID
    role: String, // User role (e.g., miner, client)
    exp: usize, // Expiration time
}

#[derive(Debug)]
struct AuthServiceImpl {
    secret_key: String,
}

impl AuthServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let secret_key = config["auth"]["secret_key"]
            .as_str()
            .unwrap_or("secret")
            .to_string();
        AuthServiceImpl { secret_key }
    }

    fn validate_role(&self, role: &str, service: &str, method: &str) -> bool {
        match (service, method) {
            ("mining_service", _) => role == "miner",
            ("validation_service", _) => role == "client" || role == "miner",
            _ => true, // Default: allow access
        }
    }
}

#[tonic::async_trait]
impl Auth for AuthServiceImpl {
    async fn authenticate(&self, request: Request<AuthenticateRequest>) -> Result<Response<AuthenticateResponse>, Status> {
        let req = request.into_inner();
        let token = decode::<Claims>(
            &req.token,
            &DecodingKey::from_secret(self.secret_key.as_bytes()),
            &Validation::default(),
        )
        .map_err(|e| Status::unauthenticated(format!("Invalid token: {}", e)))?;

        Ok(Response::new(AuthenticateResponse {
            success: true,
            user_id: token.claims.sub,
            error: "".to_string(),
        }))
    }

    async fn authorize(&self, request: Request<AuthorizeRequest>) -> Result<Response<AuthorizeResponse>, Status> {
        let req = request.into_inner();
        let token = decode::<Claims>(
            &req.user_id,
            &DecodingKey::from_secret(self.secret_key.as_bytes()),
            &Validation::default(),
        )
        .map_err(|e| Status::unauthenticated(format!("Invalid user_id: {}", e)))?;

        let allowed = self.validate_role(&token.claims.role, &req.service, &req.method);
        Ok(Response::new(AuthorizeResponse {
            allowed,
            error: if allowed { "".to_string() } else { "Access denied".to_string() },
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50060".parse().unwrap();
    let auth_service = AuthServiceImpl::new().await;

    println!("Auth service listening on {}", addr);

    Server::builder()
        .add_service(AuthServer::new(auth_service))
        .serve(addr)
        .await?;

    Ok(())
}
