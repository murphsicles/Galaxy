# Validation Service

This service implements a gRPC server for SPV proof generation and verification in the Galaxy project, using rust-sv. It integrates with block_service, storage_service, and auth_service, supporting full merkle path, difficulty validation, streaming, caching, rate limiting, logging, sharding, and metrics for continuous proof generation for BSV’s large blocks.

## Running
```bash
cd validation_service
cargo run
```
Note: Ensure block_service (localhost:50054), storage_service (localhost:50053), and auth_service (localhost:50060) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid transaction IDs, block headers, and JWT tokens in the `authorization` header. Streaming requires a client capable of handling bidirectional streams.

### GenerateSPVProof
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123"}' localhost:50057 validation.Validation/GenerateSPVProof
```
Expected response (example):
```json
{
  "success": true,
  "merkle_path": "...",
  "block_headers": ["..."],
  "error": ""
}
```

### VerifySPVProof
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123", "merkle_path": "...", "block_headers": ["..."]}' localhost:50057 validation.Validation/VerifySPVProof
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### BatchGenerateSPVProof
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txids": ["abc123", "def456"]}' localhost:50057 validation.Validation/BatchGenerateSPVProof
```
Expected response (example):
```json
{
  "results": [
    {
      "success": true,
      "merkle_path": "...",
      "block_headers": ["..."],
      "error": ""
    },
    {
      "success": true,
      "merkle_path": "...",
      "block_headers": ["..."],
      "error": ""
    }
  ]
}
```

### StreamSPVProofs
```bash
# Use a gRPC client supporting streaming
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123"}' localhost:50057 validation.Validation/StreamSPVProofs
```
Expected response (example, streamed):
```json
{
  "success": true,
  "txid": "abc123",
  "merkle_path": "...",
  "block_headers": ["..."],
  "error": ""
}
```

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50057 validation.Validation/GetMetrics
```
Expected response (example):
```json
{
  "proof_requests": 100,
  "avg_latency_ms": 10.5,
  "cache_hits": 50
}
```
