# Transaction Service

This service implements a gRPC server for BSV transaction validation and processing in the Galaxy project, using rust-sv. It integrates with storage_service for UTXO validation and supports transaction queuing and batch validation for high throughput.

## Running
```bash
cd transaction_service
cargo run
```
Note: Ensure storage_service is running on localhost:50053.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require hex-encoded BSV transactions, and storage_service must be running for UTXO validation.

### ValidateTransaction
```bash
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ValidateTransaction
```
Expected response (example):
```json
{
  "is_valid": false,
  "error": "UTXO not found"
}
```

### ProcessTransaction
```bash
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ProcessTransaction
```
Expected response (example):
```json
{
  "success": false,
  "error": "UTXO not found"
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
      "is_valid": false,
      "error": "UTXO not found"
    },
    {
      "is_valid": false,
      "error": "UTXO not found"
    }
  ]
}
```
