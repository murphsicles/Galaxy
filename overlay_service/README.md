# Overlay Service

This service implements a gRPC server for managing private blockchains (overlays) on BSV in the Galaxy project, using rust-sv. It integrates with transaction_service, block_service, and network_service.

## Running
```bash
cd overlay_service
cargo run
```
Note: Ensure transaction_service (localhost:50052), block_service (localhost:50054), and network_service (localhost:50051) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid overlay IDs and hex-encoded transactions.

### CreateOverlay
```bash
grpcurl -plaintext -d '{"overlay_id": "test_overlay"}' localhost:50056 overlay.Overlay/CreateOverlay
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### SubmitOverlayTransaction
```bash
grpcurl -plaintext -d '{"overlay_id": "test_overlay", "tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50056 overlay.Overlay/SubmitOverlayTransaction
```
Expected response (example):
```json
{
  "success": true,
  "error": ""
}
```

### GetOverlayBlock
```bash
grpcurl -plaintext -d '{"overlay_id": "test_overlay", "block_height": 0}' localhost:50056 overlay.Overlay/GetOverlayBlock
```
Expected response (example):
```json
{
  "block_hex": "",
  "error": "Block height out of range"
}
```

### BatchSubmitOverlayTransaction
```bash
grpcurl -plaintext -d '{"overlay_id": "test_overlay", "tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50056 overlay.Overlay/BatchSubmitOverlayTransaction
```
Expected response (example):
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
