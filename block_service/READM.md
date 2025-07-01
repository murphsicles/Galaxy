# Block Service

This service implements a gRPC server for block validation and assembly in the Galaxy project, using rust-sv. It integrates with transaction_service, storage_service, consensus_service, and auth_service, supporting rate limiting, logging, sharding, and metrics for BSV’s large blocks.

## Running
```bash
cd block_service
cargo run
```
Note: Ensure transaction_service (localhost:50052), storage_service (localhost:50053), consensus_service (localhost:50055), and auth_service (localhost:50060) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid hex-encoded blocks and JWT tokens in the `authorization` header.

### ValidateBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50054 block.Block/ValidateBlock
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### AssembleBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50054 block.Block/AssembleBlock
```
Expected response (example):
```json
{
  "block_hex": "...",
  "error": ""
}
```

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50054 block.Block/GetMetrics
```
Expected response (example):
```json
{
  "service_name": "block_service",
  "requests_total": 75,
  "avg_latency_ms": 12.0,
  "errors_total": 0
}
```
