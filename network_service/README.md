# Network Service

This service implements a gRPC server for BSV P2P networking in the Galaxy project, using rust-sv and a peer pool. It integrates with transaction_service for transaction validation.

## Running
```bash
cd network_service
cargo run
```
Note: Ensure transaction_service is running on localhost:50052.

## Testing
Use `grpcurl` to test the available methods. Note: Some methods require hex-encoded BSV transactions or blocks, and transaction_service must be running.

### Ping
```bash
grpcurl -plaintext -d '{"message": "Hello"}' localhost:50051 network.Network/Ping
```
Expected response:
```json
{
  "reply": "Pong: Hello"
}
```

### DiscoverPeers
```bash
grpcurl -plaintext -d '{"node_id": "galaxy_node"}' localhost:50051 network.Network/DiscoverPeers
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
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50051 network.Network/BroadcastTransaction
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### BroadcastBlock
```bash
grpcurl -plaintext -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50051 network.Network/BroadcastBlock
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```
