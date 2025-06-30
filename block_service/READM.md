# Block Service

This service implements a gRPC server for BSV block validation and assembly in the Galaxy project, using rust-sv. It integrates with transaction_service, storage_service, and consensus_service.

## Running
```bash
cd block_service
cargo run
```
Note: Ensure transaction_service (localhost:50052), storage_service (localhost:50053), and consensus_service (localhost:50055) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require hex-encoded BSV blocks or transactions.

### ValidateBlock
```bash
grpcurl -plaintext -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50054 block.Block/ValidateBlock
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
grpcurl -plaintext -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50054 block.Block/AssembleBlock
```
Expected response (example):
```json
{
  "block_hex": "...",
  "error": ""
}
```
