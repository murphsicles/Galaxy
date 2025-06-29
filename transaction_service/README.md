# Transaction Service

This service implements a gRPC server for BSV transaction validation and processing in the Galaxy project, using rust-sv.

## Running
```bash
cd transaction_service
cargo run
```

## Testing
Use `grpcurl` to test the available methods. Note: Methods require hex-encoded BSV transactions.

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
Expected response:
```json
{
  "success": true,
  "error": ""
}
```
