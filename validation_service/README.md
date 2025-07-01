# Validation Service

This service implements a gRPC server for SPV proof generation and verification in the Galaxy project, using rust-sv. It integrates with block_service and storage_service, supporting full merkle path, difficulty validation, streaming, caching, and metrics for continuous proof generation for BSV’s large blocks.

## Running
```bash
cd validation_service
cargo run
```
Note: Ensure block_service (localhost:50054) and storage_service (localhost:50053) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid transaction IDs and block headers. Streaming requires a client capable of handling bidirectional streams.

### GenerateSPVProof
```bash
grpcurl -plaintext -d '{"txid": "abc123"}' localhost:50057 validation.Validation/GenerateSPVProof
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
grpcurl -plaintext -d '{"txid": "abc123", "merkle_path": "...", "block_headers": ["..."]}' localhost:50057 validation.Validation/VerifySPVProof
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
grpcurl -plaintext -d '{"txids": ["abc123", "def456"]}' localhost:50057 validation.Validation/BatchGenerateSPVProof
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
grpcurl -plaintext -d '{"txid": "abc123"}' localhost:50057 validation.Validation/StreamSPVProofs
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
grpcurl -plaintext -d '{}' localhost:50057 validation.Validation/GetMetrics
```
Expected response (example):
```json
{
  "proof_requests": 100,
  "avg_latency_ms": 10.5,
  "cache_hits": 50
}
```
