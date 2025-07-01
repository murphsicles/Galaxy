# Authentication Service

This service implements a gRPC server for token-based authentication and role-based access control (RBAC) in the Galaxy project, using JWTs. It secures access to all services, ensuring only authorized users (e.g., miners, clients) can invoke specific methods.

## Running
```bash
cd auth_service
cargo run
```
Note: Ensure `tests/config.toml` has a valid `secret_key` under `[auth]`.

## Testing
Use `grpcurl` to test the available methods. Note: Methods require valid JWT tokens.

### Authenticate
```bash
grpcurl -plaintext -d '{"token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c2VyMSIsInJvbGUiOiJjbGllbnQiLCJleHAiOjE5MjA2NzY1MDl9.8X8z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Qz5z5z7z3Y8Q"}' localhost:50060 auth.Auth/Authenticate
```
Expected response (example):
```json
{
  "success": true,
  "user_id": "user1",
  "error": ""
}
```

### Authorize
```bash
grpcurl -plaintext -d '{"user_id": "user1", "service": "validation_service", "method": "GenerateSPVProof"}' localhost:50060 auth.Auth/Authorize
```
Expected response (example):
```json
{
  "allowed": true,
  "error": ""
}
```
