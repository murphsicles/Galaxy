# Network Service

This service implements a gRPC server for network communication in the Galaxy project, supporting BSV node networking.

## Running
```bash
cd network_service
cargo run
```

## Testing
Use `grpcurl` to test the available methods:

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
grpcurl -plaintext -d '{"node_id": "test_node"}' localhost:50051 network.Network/DiscoverPeers
```
Expected response:
```json
{
  "peer_addresses": ["node1.bsv.network:8333", "node2.bsv.network:8333"]
}
```

### BroadcastTransaction
```bash
grpcurl -plaintext -d '{"transaction": "hex_encoded_tx"}' localhost:50051 network.Network/BroadcastTransaction
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
grpcurl -plaintext -d '{"block": "hex_encoded_block"}' localhost:50051 network.Network/BroadcastBlock
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```
