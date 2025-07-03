# Validation Service

The Validation Service handles Simplified Payment Verification (SPV) proof generation and verification for Bitcoin SV (BSV), optimized from the ground-up for ultra high-throughput processing to support massive transaction volumes.

## Features

- **SPV Proof Generation**: Generates SPV proofs with `GenerateSPVProof` and `BatchGenerateSPVProof` RPCs.
- **SPV Proof Streaming**: Streams SPV proofs with `StreamSPVProofs` RPC.
- **SPV Proof Verification**: Verifies SPV proofs with `VerifySPVProof` RPC.
- **Metrics**: Monitors performance with Prometheus metrics via `GetMetrics` RPC, including `requests_total`, `avg_latency_ms`, `errors_total`, `cache_hits`, `alert_count`, and `index_throughput`.

## Configuration

The service uses `tests/config.toml` for configuration:
- **Sharding**: Configures `shard_id` and `shard_count` for distributed processing.
- **Metrics**: Enables Prometheus endpoint (`enable_prometheus`, port: `9090`) and sets alert thresholds (`alert_threshold`) and log level (`log_level`).

Example `tests/config.toml`:
```toml
[sharding]
shard_id = 0
shard_count = 4

[metrics]
enable_prometheus = true
prometheus_port = 9090
alert_threshold = 5
log_level = "info"
```

## Dependencies

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`, `futures`, `async-stream`, `lru`.
- **Internal Services**: `block_service` (`:50054`), `storage_service` (`:50053`), `auth_service` (`:50060`), `alert_service` (`:50061`).
- **Proto Files**: `validation.proto`, `block.proto`, `storage.proto`, `auth.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `validation_requests_total`: Total SPV proof requests.
- `validation_latency_ms`: Average proof generation latency (ms).
- `validation_alert_count`: Total alerts sent.
- `validation_cache_hits`: Cache hit count for SPV proofs.
- `errors_total`: Total errors (placeholder, currently 0).
- `index_throughput`: Indexed items per second (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: No direct block size limit enforced; relies on `block_service` (temporary 32GB limit) and `consensus_service`.
- **OP_RETURN**: No direct OP_RETURN limit enforced; relies on `transaction_service` and `consensus_service` (4.3GB limit).
- **Transaction Size**: No direct transaction size limit enforced; relies on `consensus_service` (temporary 32GB limit).
- The service supports BSV’s massive transaction and block sizes through integration with other services.

## Usage

1. Start the service:
   ```bash
   cargo run --bin validation_service
   ```
   Listens on `[::1]:50057`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"txid": "abc123..."}' [::1]:50057 validation.Validation/GenerateSPVProof
   grpcurl -plaintext -d '{"txid": "abc123...", "merkle_path": "def456...", "block_headers": ["01000000..."]}' [::1]:50057 validation.Validation/VerifySPVProof
   grpcurl -plaintext -d '{"txids": ["abc123...", "def456..."]}' [::1]:50057 validation.Validation/BatchGenerateSPVProof
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify SPV proof generation and verification.

## Notes

- Uses an LRU cache for SPV proof storage, tracked via `validation_cache_hits` metric.
- Alerts are sent via `alert_service` for failures (e.g., block fetch, merkle path generation, verification errors).
- Designed for high-throughput, supporting BSV’s massive transaction and block sizes through validation by other services.

---
*Last updated: 2025-07-03*
