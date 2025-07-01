# Index Service

This service implements a gRPC server for transaction and block indexing in the Galaxy project, using `sled` as a placeholder storage backend. It supports efficient querying for BSV’s high transaction volume, integrating with `auth_service` for secure access.

## Running
```bash
cd index_service
cargo run
```
Note: Ensure `auth_service` (localhost:50060) is running. Configure `[index]` in `tests/config.toml` for storage paths.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid hex-encoded transactions/blocks and JWT tokens in the `authorization` header.

### IndexTransaction
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50062 index.Index/IndexTransaction
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### QueryTransaction
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"txid": "abc123"}' localhost:50062 index.Index/QueryTransaction
```
Expected response (example):
```json
{
  "tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff",
  "error": ""
}
```

### BatchIndexTransactions
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50062 index.Index/BatchIndexTransactions
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

### IndexBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50062 index.Index/IndexBlock
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### QueryBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hash": "abc123"}' localhost:50062 index.Index/QueryBlock
```
Expected response (example):
```json
{
  "block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff",
  "error": ""
}
```

### BatchIndexBlocks
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hexes": ["01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff", "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"]}' localhost:50062 index.Index/BatchIndexBlocks
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
