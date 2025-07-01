# Storage Service

This service implements a gRPC server for UTXO storage in the Galaxy project, using Tiger Beetle (currently a HashMap fallback for testing). It integrates with auth_service, supporting rate limiting, logging, sharding, and metrics for high-throughput BSV operations.

## Running
```bash
cd storage_service
cargo run
```
Note: Ensure auth_service (localhost:50060) is running. Tiger Beetle integration requires a running server (e.g., `tigerbeetle start --cluster=0 --replica=0`).

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid TXID, vout, and JWT tokens in the `authorization` header.

### QueryUtxo
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123", "vout": 0}' localhost:50053 storage.Storage/QueryUtxo
```
Expected response (example, if UTXO exists):
```json
{
  "exists": true,
  "script_pubkey": "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac",
  "amount": 1000000,
  "error": ""
}
```

### AddUtxo
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123", "vout": 0, "script_pubkey": "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac", "amount": 1000000}' localhost:50053 storage.Storage/AddUtxo
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### RemoveUtxo
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123", "vout": 0}' localhost:50053 storage.Storage/RemoveUtxo
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### BatchAddUtxo
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"utxos": [{"txid": "abc123", "vout": 0, "script_pubkey": "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac", "amount": 1000000}, {"txid": "def456", "vout": 1, "script_pubkey": "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac", "amount": 2000000}]}' localhost:50053 storage.Storage/BatchAddUtxo
```
Expected response:
```json
{
  "results": [
    {
      "success": true,
      "error": ""
    },
    {
      "success": true,
      "error": ""
    }
  ]
}
```

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50053 storage.Storage/GetMetrics
```
Expected response (example):
```json
{
  "service_name": "storage_service",
  "requests_total": 200,
  "avg_latency_ms": 5.0,
  "errors_total": 0
}
```
