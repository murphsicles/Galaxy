// validation_service/src/service.rs
use bincode::{deserialize, serialize};
use sled::{Db, IVec};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, error};
use serde::{Deserialize, Serialize};
use sv::transaction::Transaction as SvTx;
use sv::merkle::MerklePath;
use sv::block::Header;
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTxRequest {
    tx: SvTx,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateTxResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofRequest {
    proof: ProofBundle,
}

#[derive(Serialize, Deserialize, Debug)]
struct ValidateProofResponse {
    success: bool,
    error: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProofBundle {
    pub tx: SvTx,
    pub path: MerklePath,
    pub header: Header,
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationRequestType {
    ValidateTx(ValidateTxRequest),
    ValidateProof(ValidateProofRequest),
}

#[derive(Serialize, Deserialize, Debug)]
enum ValidationResponseType {
    ValidateTx(ValidateTxResponse),
    ValidateProof(ValidateProofResponse),
}

pub struct ValidationService {
    db: Arc<Mutex<Db>>,
    block_service_addr: String,
}

impl ValidationService {
    pub fn new(db_path: &str) -> Self {
        let db = sled::open(db_path).expect("Failed to open sled db for validation");
        Self {
            db: Arc::new(Mutex::new(db)),
            block_service_addr: "127.0.0.1:50054".to_string(),
        }
    }

    pub async fn validate_tx(&self, tx: &SvTx) -> Result<(), String> {
        // Basic validation: Check tx format, signatures, and inputs
        if !tx.is_valid() {
            return Err("Invalid transaction format or signatures".to_string());
        }

        // Placeholder: Check inputs against UTXO set (requires storage_service or db query)
        // For now, assume valid if format is correct
        info!("Validated transaction: {}", tx.txid());
        Ok(())
    }

    pub async fn validate_proof(&self, proof: &ProofBundle) -> Result<(), String> {
        // Verify merkle path against header
        let computed_root = proof.path.compute_root(&proof.tx.txid())
            .map_err(|e| format!("Failed to compute merkle root: {}", e))?;
        if computed_root != proof.header.merkle_root {
            return Err("Merkle proof does not match block header".to_string());
        }

        // Verify header exists in chain
        let db = self.db.lock().await;
        let key = proof.header.hash().to_string().as_bytes();
        if db.get(key).map_err(|e| format!("Sled get error: {}", e))?.is_none() {
            return Err("Block header not found in chain".to_string());
        }

        info!("Validated merkle proof for tx: {} in block: {}", proof.tx.txid(), proof.header.hash());
        Ok(())
    }

    pub async fn handle_request(&self, request: &[u8]) -> Vec<u8> {
        let req: ValidationRequestType = match deserialize(request) {
            Ok(req) => req,
            Err(e) => {
                error!("Deserialization error: {}", e);
                let resp = ValidationResponseType::ValidateTx(ValidateTxResponse {
                    success: false,
                    error: e.to_string(),
                });
                return serialize(&resp).unwrap();
            }
        };

        match req {
            ValidationRequestType::ValidateTx(req) => {
                match self.validate_tx(&req.tx).await {
                    Ok(_) => {
                        let resp = ValidationResponseType::ValidateTx(ValidateTxResponse {
                            success: true,
                            error: String::new(),
                        });
                        serialize(&resp).unwrap()
                    }
                    Err(e) => {
                        error!("Transaction validation error: {}", e);
                        let resp = ValidationResponseType::ValidateTx(ValidateTxResponse {
                            success: false,
                            error: e,
                        });
                        serialize(&resp).unwrap()
                    }
                }
            }
            ValidationRequestType::ValidateProof(req) => {
                match self.validate_proof(&req.proof).await {
                    Ok(_) => {
                        let resp = ValidationResponseType::ValidateProof(ValidateProofResponse {
                            success: true,
                            error: String::new(),
                        });
                        serialize(&resp).unwrap()
                    }
                    Err(e) => {
                        error!("Proof validation error: {}", e);
                        let resp = ValidationResponseType::ValidateProof(ValidateProofResponse {
                            success: false,
                            error: e,
                        });
                        serialize(&resp).unwrap()
                    }
                }
            }
        }
    }

    pub async fn store_header(&self, header: &Header) -> Result<(), String> {
        let db = self.db.lock().await;
        let key = header.hash().to_string().as_bytes();
        let serialized = serialize(header).map_err(|e| format!("Serialization error: {}", e))?;
        db.insert(key, serialized).map_err(|e| format!("Sled insert error: {}", e))?;
        db.flush().await.map_err(|e| format!("Sled flush error: {}", e))?;
        info!("Stored block header: {}", header.hash());
        Ok(())
    }
}
