# Transaction Service

This service implements a gRPC server for BSV transaction validation and processing in the Galaxy project, using rust-sv. It integrates with storage_service for UTXO validation, consensus_service for consensus rules, and supports transaction queuing and batch validation.

## Running
```bash
cd transaction_service
cargo run
```
Note: Ensure storage_service (localhost:50053) and consensus_service (localhost:50055) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require hex-encoded BSV transactions, and dependent services must be running.

### ValidateTransaction
```bash
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ValidateTransaction
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### ProcessTransaction
```bash
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ProcessTransaction
```
Expected response (example):
```json
{
  "success": true,
  "error": ""
}
```

### BatchValidateTransaction
```bash
grpcurl -plaintext -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50052 transaction.Transaction/BatchValidateTransaction
```
Expected response (example):
```json
{
  "results": [
    {
      "is_valid": true,
      "error": ""
    },
    {
      "is_valid": true,
      "error": ""
    }
  ]
}
```
