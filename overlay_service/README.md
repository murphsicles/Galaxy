# Overlay Service

This service implements a gRPC server for managing private blockchains (overlays) on BSV in the Galaxy project, using rust-sv. It integrates with `transaction_service`, `block_service`, `network_service`, `index_service`, `alert_service`, and `auth_service`, supporting block assembly, persistent storage with sled, streaming, rate limiting, logging, sharding, and metrics.

## Running
```bash
cd overlay_service
cargo run
```
Note: Ensure `transaction_service` (localhost:50052), `block_service` (localhost:50054), `network_service` (localhost:50051), `index_service` (localhost:50062), `alert_service` (localhost:50061), and `auth_service` (localhost:50060) are running.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid overlay IDs, hex-encoded transactions, and JWT tokens in the `authorization` header.

### CreateOverlay
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay"}' localhost:50056 overlay.Overlay/CreateOverlay
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
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay", "tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50056 overlay.Overlay/SubmitOverlayTransaction
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
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay", "block_height": 0}' localhost:50056 overlay.Overlay/GetOverlayBlock
```
Expected response (example):
```json
{
  "block_hex": "...",
  "error": ""
}
```

### BatchSubmitOverlayTransaction
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay", "tx_hexes": ["01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff", "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"]}' localhost:50056 overlay.Overlay/BatchSubmitOverlayTransaction
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

### StreamOverlayTransactions
```bash
# Use a gRPC client supporting streaming
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay", "tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50056 overlay.Overlay/StreamOverlayTransactions
```
Expected response (example, streamed):
```json
{
  "success": true,
  "tx_hex": "...",
  "error": ""
}
```

### IndexOverlayTransaction
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay", "tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50056 overlay.Overlay/IndexOverlayTransaction
```
Expected response (example):
```json
{
  "success": true,
  "error": ""
}
```

### QueryOverlayBlock
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"overlay_id": "test_overlay", "block_height": 0}' localhost:50056 overlay.Overlay/QueryOverlayBlock
```
Expected response (example):
```json
{
  "block_hex": "...",
  "error": ""
}
```

### GetMetrics
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{}' localhost:50056 overlay.Overlay/GetMetrics
```
Expected response (example):
```json
{
  "service_name": "overlay_service",
  "requests_total": 200,
  "avg_latency_ms": 15.0,
  "errors_total": 0
}
```
