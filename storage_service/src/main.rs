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
    utxo_db: Arc<Mutex<HashMap<(String, u32), (String, u64)>>>, // Fallback for testing
    tb_client: Option<Client>, // Tiger Beetle client
}

impl StorageServiceImpl {
    async fn new() -> Self {
        let config_str = include_str!("../../tests/config.toml");
        let config: toml::Value = toml::from_str(config_str).expect("Failed to parse config");
        let tb_address = config["testnet"]["tigerbeetle_address"]
            .as_str()
            .unwrap()
            .to_string();

        // Initialize Tiger Beetle client with keep-alive
        let tb_client = Client::new(Config {
            cluster_id: 0,
            replica_id: 0,
            addresses: vec![tb_address],
            keep_alive: true, // Enable connection keep-alive
        }).await.ok();

        let utxo_db = Arc::new(Mutex::new(HashMap::new()));
        StorageServiceImpl { utxo_db, tb_client }
    }

    async fn query_utxo_tb(&self, txid: &str, vout: u32) -> Result<Option<(String, u64)>, String> {
        // Placeholder for Tiger Beetle query
        Ok(None)
    }

    async fn add_utxo_tb(&self, txid: &str, vout: u32, script_pubkey: &str, amount: u64) -> Result<(), String> {
        // Placeholder for Tiger Beetle write
        Ok(())
    }

    async fn remove_utxo_tb(&self, txid: &str, vout: u32) -> Result<(), String> {
        // Placeholder for Tiger Beetle delete
        Ok(())
    }

    async fn batch_add_utxo_tb(&self, utxos: Vec<(String, u32, String, u64)>) -> Result<Vec<bool>, String> {
        // Placeholder for Tiger Beetle batch write
        Ok(vec![true; utxos.len()])
    }
}

#[tonic::async_trait]
impl Storage for StorageServiceImpl {
    async fn query_utxo(&self, request: Request<QueryUtxoRequest>) -> Result<Response<QueryUtxoResponse>, Status> {
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            match self.query_utxo_tb(&req.txid, req.vout).await {
                Ok(Some((script_pubkey, amount))) => {
                    return Ok(Response::new(QueryUtxoResponse {
                        exists: true,
                        script_pubkey,
                        amount,
                        error: "".to_string(),
                    }));
                }
                Ok(None) => {
                    return Ok(Response::new(QueryUtxoResponse {
                        exists: false,
                        script_pubkey: "".to_string(),
                        amount: 0,
                        error: "UTXO not found".to_string(),
                    }));
                }
                Err(e) => {
                    return Ok(Response::new(QueryUtxoResponse {
                        exists: false,
                        script_pubkey: "".to_string(),
                        amount: 0,
                        error: e,
                    }));
                }
            }
        }

        let utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);
        if let Some((script_pubkey, amount)) = utxo_db.get(&key) {
            Ok(Response::new(QueryUtxoResponse {
                exists: true,
                script_pubkey: script_pubkey.clone(),
                amount: *amount,
                error: "".to_string(),
            }))
        } else {
            Ok(Response::new(QueryUtxoResponse {
                exists: false,
                script_pubkey: "".to_string(),
                amount: 0,
                error: "UTXO not found".to_string(),
            }))
        }
    }

    async fn add_utxo(&self, request: Request<AddUtxoRequest>) -> Result<Response<AddUtxoResponse>, Status> {
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            if let Err(e) = self.add_utxo_tb(&req.txid, req.vout, &req.script_pubkey, req.amount).await {
                return Ok(Response::new(AddUtxoResponse {
                    success: false,
                    error: e,
                }));
            }
            return Ok(Response::new(AddUtxoResponse {
                success: true,
                error: "".to_string(),
            }));
        }

        let mut utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);
        utxo_db.insert(key, (req.script_pubkey, req.amount));
        Ok(Response::new(AddUtxoResponse {
            success: true,
            error: "".to_string(),
        }))
    }

    async fn remove_utxo(&self, request: Request<RemoveUtxoRequest>) -> Result<Response<RemoveUtxoResponse>, Status> {
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            if let Err(e) = self.remove_utxo_tb(&req.txid, req.vout).await {
                return Ok(Response::new(RemoveUtxoResponse {
                    success: false,
                    error: e,
                }));
            }
            return Ok(Response::new(RemoveUtxoResponse {
                success: true,
                error: "".to_string(),
            }));
        }

        let mut utxo_db = self.utxo_db.lock().await;
        let key = (req.txid, req.vout);
        let success = utxo_db.remove(&key).is_some();
        Ok(Response::new(RemoveUtxoResponse {
            success,
            error: if success { "".to_string() } else { "UTXO not found".to_string() },
        }))
    }

    async fn batch_add_utxo(&self, request: Request<BatchAddUtxoRequest>) -> Result<Response<BatchAddUtxoResponse>, Status> {
        let req = request.into_inner();
        if let Some(tb_client) = &self.tb_client {
            let utxos = req.utxos.into_iter()
                .map(|u| (u.txid, u.vout, u.script_pubkey, u.amount))
                .collect();
            let results = self.batch_add_utxo_tb(utxos).await
                .map_err(|e| Status::internal(format!("Batch add failed: {}", e)))?;
            return Ok(Response::new(BatchAddUtxoResponse {
                results: results.into_iter().map(|success| AddUtxoResponse {
                    success,
                    error: if success { "".to_string() } else { "Failed to add UTXO".to_string() },
                }).collect(),
            }));
        }

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
        Ok(Response::new(BatchAddUtxoResponse { results }))
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
