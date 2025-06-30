# Consensus Service

This service implements a gRPC server for BSV consensus rule validation in the Galaxy project, using rust-sv.

## Running
```bash
cd consensus_service
cargo run
```

## Testing
Use `grpcurl` to test the available methods. Note: Methods require hex-encoded BSV blocks or transactions.

### ValidateBlockConsensus
```bash
grpcurl -plaintext -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50055 consensus.Consensus/ValidateBlockConsensus
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### ValidateTransactionConsensus
```bash
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50055 consensus.Consensus/ValidateTransactionConsensus
```
Expected response (example):
```json
{
  "is_valid": true,
  "error": ""
}
```

### BatchValidateTransactionConsensus
```bash
grpcurl -plaintext -d '{"tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50055 consensus.Consensus/BatchValidateTransactionConsensus
```
Expected response (example):
```json
{
  "results": [
    {
      "is_valid": true,
      "error": ""
    },
    {
      "is_valid": true,
      "error": ""
    }
  ]
}
```
