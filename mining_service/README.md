# Mining Service

This service implements a gRPC server for BSV block mining in the Galaxy project, using rust-sv. It integrates with block_service, network_service, and auth_service, supporting full proof-of-work validation, transaction selection, streaming, rate limiting, logging, sharding, and metrics.

## Running
```bash
cd mining_service
cargo run
```
Note: Ensure block_service (localhost:50054), network_service (localhost:50051), and auth_service (localhost:50060) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid miner IDs, hex-encoded blocks, and JWT tokens in the `authorization` header.

### GetMiningWork
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtaW5lcjEiLCJyb2xlIjoibWluZXIiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"miner_id": "miner1"}' localhost:50058 mining.Mining/GetMiningWork
```
Expected response (example):
```json
{
  "block_template": "...",
  "target_difficulty": 486604799,
  "error": ""
}
```

### SubmitMinedBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtaW5lcjEiLCJyb2xlIjoibWluZXIiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50058 mining.Mining/SubmitMinedBlock
```
Expected response (example):
```json
{
  "success": true,
  "error": ""
}
```

### BatchGetMiningWork
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtaW5lcjEiLCJyb2xlIjoibWluZXIiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"miner_ids": ["miner1", "miner2"]}' localhost:50058 mining.Mining/BatchGetMiningWork
```
Expected response (example):
```json
{
  "results": [
    {
      "block_template": "...",
      "target_difficulty": 486604799,
      "error": ""
    },
    {
      "block_template": "...",
      "target_difficulty": 486604799,
      "error": ""
    }
  ]
}
```

### StreamMiningWork
```bash
# Use a gRPC client supporting streaming
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtaW5lcjEiLCJyb2xlIjoibWluZXIiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"miner_id": "miner1"}' localhost:50058 mining.Mining/StreamMiningWork
```
Expected response (example, streamed):
```json
{
  "block_template": "...",
  "target_difficulty": 486604799,
  "error": ""
}
```

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJtaW5lcjEiLCJyb2xlIjoibWluZXIiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50058 mining.Mining/GetMetrics
```
Expected response (example):
```json
{
  "work_requests": 150,
  "avg_latency_ms": 20.0,
  "blocks_submitted": 5
}
```
