# Validation Service

This service implements a gRPC server for SPV proof generation and verification in the Galaxy project, using rust-sv. It integrates with block_service and storage_service.

## Running
```bash
cd validation_service
cargo run
```
Note: Ensure block_service (localhost:50054) and storage_service (localhost:50053) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid transaction IDs.

### GenerateSPVProof
```bash
grpcurl -plaintext -d '{"txid": "abc123"}' localhost:50057 validation.Validation/GenerateSPVProof
```
Expected response (example):
```json
{
  "success": true,
  "merkle_path": "0000000000000000000000000000000000000000000000000000000000000000",
  "block_headers": ["..."],
  "error": ""
}
```

### VerifySPVProof
```bash
grpcurl -plaintext -d '{"txid": "abc123", "merkle_path": "0000000000000000000000000000000000000000000000000000000000000000", "block_headers": ["..."]}' localhost:50057 validation.Validation/VerifySPVProof
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
      "merkle_path": "0000000000000000000000000000000000000000000000000000000000000000",
      "block_headers": ["..."],
      "error": ""
    },
    {
      "success": true,
      "merkle_path": "0000000000000000000000000000000000000000000000000000000000000000",
      "block_headers": ["..."],
      "error": ""
    }
  ]
}
```
