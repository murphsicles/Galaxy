# Transaction Service

The Transaction Service handles transaction validation, processing, and indexing for Bitcoin SV (BSV), optimized from the ground-up for ultra high-throughput processing.

## Features

- **Transaction Validation**: Validates transactions with `ValidateTransaction` and `BatchValidateTransaction` RPCs, enforcing consensus rules.
- **Transaction Processing**: Processes transactions with `ProcessTransaction` RPC, including validation, indexing, and queuing.
- **Transaction Indexing**: Indexes transactions with `IndexTransaction` RPC.
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

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`, `futures`, `async-channel`.
- **Internal Services**: `storage_service` (`:50053`), `consensus_service` (`:50055`), `auth_service` (`:50060`), `index_service` (`:50062`), `alert_service` (`:50061`).
- **Proto Files**: `transaction.proto`, `storage.proto`, `consensus.proto`, `auth.proto`, `index.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `transaction_requests_total`: Total transaction requests.
- `transaction_latency_ms`: Average transaction processing latency (ms).
- `transaction_alert_count`: Total alerts sent.
- `transaction_index_throughput`: Indexed transactions per second.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).

## Consensus and Size Limits

- **Transaction Size**: Enforces a temporary 32GB limit via `consensus_service`.
- **OP_RETURN**: Enforces a 4.3GB limit via `consensus_service`.
- The service relies on `consensus_service` for transaction validation rules, ensuring compatibility with BSV’s large transaction and OP_RETURN capabilities.

## Usage

1. Start the service:
   ```bash
   cargo run --bin transaction_service
   ```
   Listens on `[::1]:50052`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"tx_hex": "01000000..."}' [::1]:50052 transaction.Transaction/ValidateTransaction
   grpcurl -plaintext -d '{"tx_hex": "01000000..."}' [::1]:50052 transaction.Transaction/ProcessTransaction
   grpcurl -plaintext -d '{"tx_hexes": ["01000000...", "02000000..."]}' [::1]:50052 transaction.Transaction/BatchValidateTransaction
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify transaction validation, processing, and indexing.

## Notes

- The service uses an async channel for transaction queuing.
- Alerts are sent via `alert_service` for failures (e.g., validation, indexing, queuing).
- Designed for high-throughput, supporting BSV’s massive transaction sizes (up to 32GB) and OP_RETURN data (up to 4.3GB) through `consensus_service` validation.

---
*Last updated: 2025-07-03*
