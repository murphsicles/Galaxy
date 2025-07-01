# Alert Service

This service implements a gRPC server for network health monitoring and notifications in the Galaxy project. It supports sending alerts for critical events (e.g., consensus violations, performance issues) and subscribing to alert streams, integrating with `auth_service` for secure access.

## Running
```bash
cd alert_service
cargo run
```
Note: Ensure `auth_service` (localhost:50060) is running. Configure `[alert]` in `tests/config.toml` for severity thresholds and event types.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid JWT tokens in the `authorization` header.

### SendAlert
```bash
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"event_type": "consensus_violation", "message": "Block size exceeds limit", "severity": 3}' localhost:50061 alert.Alert/SendAlert
```
Expected response:
```json
{
  "success": true,
  "error": ""
}
```

### SubscribeToAlerts
```bash
# Use a gRPC client supporting streaming
grpcurl -plaintext -H "authorization: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q" -d '{"event_type": "consensus_violation"}' localhost:50061 alert.Alert/SubscribeToAlerts
```
Expected response (example, streamed):
```json
{
  "event_type": "consensus_violation",
  "message": "Block size exceeds limit",
  "severity": 3,
  "timestamp": 1625097600
}
```
