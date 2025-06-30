use tonic::{transport::Server, Request, Response, Status};
use storage::storage_server::{Storage, StorageServer};
use storage::{
    QueryUtxoRequest, QueryUtxoResponse, AddUtxoRequest, AddUtxoResponse,
    RemoveUtxoRequest, RemoveUtxoResponse, BatchAddUtxoRequest, BatchAddUtxoResponse
};
use tigerbeetle::client::{Client, Config};
use std::collections::HashMap;
use tokio::sync::Mutex;
use std::sync::Arc;
use toml;

tonic::include_proto!("storage");

#[derive(Debug)]
struct StorageServiceImpl {
    utxo_db: Arc<Mutex<HashMap<(String, u32), (String, u64)>>>,
    // TODO: Enable Tiger Beetle client when CLI access is available
    // tb_client: Option<Client>,
}

impl StorageServiceImpl {
    async fn new() -> Self {
        // Load Tiger Beetle address from config
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let tb_address = config["testnet"]["tigerbeetle_address"]
            .as_str()
            .unwrap()
            .to_string();

        // TODO: Initialize Tiger Beetle client when CLI access is available
        // let tb_client = Client::new(Config {
        //     cluster_id: 0,
        //     replica_id: 0,
        //     addresses: vec![tb_address],
        // }).await.ok();
        let utxo_db = Arc::new(Mutex::new(HashMap::new()));
        StorageServiceImpl { utxo_db }
    }
}

#[tonic::async_trait]
impl Storage for StorageServiceImpl {
    async fn query_utxo(&self, request: Request<QueryUtxoRequest>) -> Result<Response<QueryUtxoResponse>, Status> {
        let req = request.into_inner();
        let utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);

        if let Some((script_pubkey, amount)) = utxo_db.get(&key) {
            let reply = QueryUtxoResponse {
                exists: true,
                script_pubkey: script_pubkey.clone(),
                amount: *amount,
                error: "".to_string(),
            };
            Ok(Response::new(reply))
        } else {
            let reply = QueryUtxoResponse {
                exists: false,
                script_pubkey: "".to_string(),
                amount: 0,
                error: "UTXO not found".to_string(),
            };
            Ok(Response::new(reply))
        }
    }

    async fn add_utxo(&self, request: Request<AddUtxoRequest>) -> Result<Response<AddUtxoResponse>, Status> {
        let req = request.into_inner();
        let mut utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);

        utxo_db.insert(key, (req.script_pubkey, req.amount));

        let reply = AddUtxoResponse {
            success: true,
            error: "".to_string(),
        };
        Ok(Response::new(reply))
    }

    async fn remove_utxo(&self, request: Request<RemoveUtxoRequest>) -> Result<Response<RemoveUtxoResponse>, Status> {
        let req = request.into_inner();
        let mut utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);

        let success = utxo_db.remove(&key).is_some();

        let reply = RemoveUtxoResponse {
            success,
            error: if success { "".to_string() } else { "UTXO not found".to_string() },
        };
        Ok(Response::new(reply))
    }

    async fn batch_add_utxo(&self, request: Request<BatchAddUtxoRequest>) -> Result<Response<BatchAddUtxoResponse>, Status> {
        let req = request.into_inner();
        let mut utxo_db = self.utxo_db.lock().await;
        let mut results = vec![];

        for utxo in req.utxos {
            let key = (utxo.txid, utxo.vout);
            utxo_db.insert(key, (utxo.script_pubkey, utxo.amount));
            results.push(AddUtxoResponse {
                success: true,
                error: "".to_string(),
            });
        }

        let reply = BatchAddUtxoResponse { results };
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50053".parse().unwrap();
    let storage_service = StorageServiceImpl::new().await;

    println!("Storage service listening on {}", addr);

    Server::builder()
        .add_service(StorageServer::new(storage_service))
        .serve(addr)
        .await?;

    Ok(())
}
