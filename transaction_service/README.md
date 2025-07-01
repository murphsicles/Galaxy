# Transaction Service

This service implements a gRPC server for transaction validation and queuing in the Galaxy project, using rust-sv. It integrates with storage_service, consensus_service, and auth_service, supporting rate limiting, logging, sharding, and metrics for BSV transactions.

## Running
```bash
cd transaction_service
cargo run
```
Note: Ensure storage_service (localhost:50053), consensus_service (localhost:50055), and auth_service (localhost:50060) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid hex-encoded transactions and JWT tokens in the `authorization` header.

### ValidateTransaction
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ValidateTransaction
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
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ProcessTransaction
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
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50052 transaction.Transaction/BatchValidateTransaction
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

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50052 transaction.Transaction/GetMetrics
```
Expected response (example):
```json
{
  "service_name": "transaction_service",
  "requests_total": 100,
  "avg_latency_ms": 8.0,
  "errors_total": 0
}
```
