# Storage Service

This service implements a gRPC server for UTXO storage in the Galaxy project, using Tiger Beetle (placeholder HashMap for now).

## Running
```bash
cd storage_service
cargo run
```

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid TXID and vout for UTXO operations.

### QueryUtxo
```bash
grpcurl -plaintext -d '{"txid": "abc123", "vout": 0}' localhost:50053 storage.Storage/QueryUtxo
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
grpcurl -plaintext -d '{"txid": "abc123", "vout": 0, "script_pubkey": "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac", "amount": 1000000}' localhost:50053 storage.Storage/AddUtxo
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
grpcurl -plaintext -d '{"txid": "abc123", "vout": 0}' localhost:50053 storage.Storage/RemoveUtxo
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```
