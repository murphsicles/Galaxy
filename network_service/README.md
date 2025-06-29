# Network Service

This service implements a gRPC server for network communication in the Galaxy project.

## Running
```bash
cd network_service
cargo run
```

## Testing
Use `grpcurl` to test the `Ping` method:
```bash
grpcurl -plaintext -d '{"message": "Hello"}' localhost:50051 network.Network/Ping
```

Expected response:
```json
{
  "reply": "Pong: Hello"
}
```
