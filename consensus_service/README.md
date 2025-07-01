# Consensus Service

This service implements a gRPC server for enforcing BSV consensus rules in the Galaxy project, using rust-sv. It integrates with auth_service, supporting rate limiting, logging, sharding, and metrics for transaction and block validation.

## Running
```bash
cd consensus_service
cargo run
```
Note: Ensure auth_service (localhost:50060) is running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid hex-encoded transactions/blocks and JWT tokens in the `authorization` header.

### ValidateBlockConsensus
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50055 consensus.Consensus/ValidateBlockConsensus
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### ValidateTransactionConsensus
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50055 consensus.Consensus/ValidateTransactionConsensus
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### BatchValidateTransactionConsensus
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50055 consensus.Consensus/BatchValidateTransactionConsensus
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
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50055 consensus.Consensus/GetMetrics
```
Expected response (example):
```json
{
  "service_name": "consensus_service",
  "requests_total": 150,
  "avg_latency_ms": 7.0,
  "errors_total": 0
}
```
