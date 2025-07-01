# Network Service

This service implements a gRPC server for P2P networking in the Galaxy project, using rust-sv. It integrates with transaction_service, block_service, and auth_service, supporting peer management, transaction/block broadcasting, rate limiting, logging, sharding, and metrics for BSV’s testnet.

## Running
```bash
cd network_service
cargo run
```
Note: Ensure transaction_service (localhost:50052), block_service (localhost:50054), and auth_service (localhost:50060) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid JWT tokens in the `authorization` header.

### Ping
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"message": "Hello"}' localhost:50051 network.Network/Ping
```
Expected response:
```json
{
  "reply": "Pong: Hello"
}
```

### DiscoverPeers
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"node_id": "galaxy_node"}' localhost:50051 network.Network/DiscoverPeers
```
Expected response (example):
```json
{
  "peer_addresses": ["testnet-seed.bitcoin.sipa.be:18333", "testnet-seed.bsv.io:18333"],
  "error": ""
}
```

### BroadcastTransaction
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50051 network.Network/BroadcastTransaction
```
Expected response (example):
```json
{
  "success": true,
  "error": ""
}
```

### BroadcastBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50051 network.Network/BroadcastBlock
```
Expected response (example):
```json
{
  "success": true,
  "error": ""
}
```

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50051 network.Network/GetMetrics
```
Expected response (example):
```json
{
  "service_name": "network_service",
  "requests_total": 50,
  "avg_latency_ms": 5.0,
  "errors_total": 0
}
```
