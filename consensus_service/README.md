# Consensus Service

The Consensus Service enforces consensus rules for Bitcoin SV (BSV) transactions and blocks, optimized from the ground-up for ultra high-throughput processing to support massive transaction volumes.

## Features

- **Block Consensus Validation**: Validates blocks with `ValidateBlockConsensus` RPC, enforcing a temporary 32GB block size limit.
- **Transaction Consensus Validation**: Validates transactions with `ValidateTransactionConsensus` and `BatchValidateTransactionConsensus` RPCs, enforcing a 4.3GB OP_RETURN limit and 32GB transaction size limit.
- **Metrics**: Monitors performance with Prometheus metrics via `GetMetrics` RPC, including `requests_total`, `avg_latency_ms`, `errors_total`, `cache_hits`, `alert_count`, and `index_throughput`.

## Configuration

The service uses `tests/config.toml` for configuration:
- **Sharding**: Configures `shard_id` and `shard_count` for distributed processing.
- **Overlay Consensus**: Sets `restrict_op_return` and `max_op_return_size` (default: 4,294,967,296 bytes, 4.3GB).
- **Metrics**: Enables Prometheus endpoint (`enable_prometheus`, port: `9090`) and sets alert thresholds (`alert_threshold`) and log level (`log_level`).

Example `tests/config.toml`:
```toml
[sharding]
shard_id = 0
shard_count = 4

[overlay_consensus]
restrict_op_return = true
max_op_return_size = 4294967296

[metrics]
enable_prometheus = true
prometheus_port = 9090
alert_threshold = 5
log_level = "info"
```

## Dependencies

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`.
- **Internal Services**: `auth_service` (`:50060`), `alert_service` (`:50061`).
- **Proto Files**: `consensus.proto`, `auth.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `consensus_requests_total`: Total consensus requests.
- `consensus_latency_ms`: Average consensus request latency (ms).
- `consensus_alert_count`: Total alerts sent.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).
- `index_throughput`: Indexed items per second (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: Enforces a temporary 32GB limit in `validate_block_rules`.
- **OP_RETURN**: Enforces a 4.3GB limit in `validate_transaction_rules`, configurable via `tests/config.toml`.
- **Transaction Size**: Enforces a temporary 32GB limit in `validate_transaction_rules`.
- The service ensures compatibility with BSV’s massive block and OP_RETURN capabilities.

## Usage

1. Start the service:
   ```bash
   cargo run --bin consensus_service
   ```
   Listens on `[::1]:50055`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"block_hex": "01000000..."}' [::1]:50055 consensus.Consensus/ValidateBlockConsensus
   grpcurl -plaintext -d '{"tx_hex": "01000000..."}' [::1]:50055 consensus.Consensus/ValidateTransactionConsensus
   grpcurl -plaintext -d '{"tx_hexes": ["01000000...", "02000000..."]}' [::1]:50055 consensus.Consensus/BatchValidateTransactionConsensus
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify block and transaction validation.

## Notes

- Alerts are sent via `alert_service` for failures (e.g., block size, transaction size, OP_RETURN size, non-standard script).
- Designed for high-throughput, supporting BSV’s massive block sizes (up to 32GB temporarily) and OP_RETURN data (up to 4.3GB).

---
*Last updated: 2025-07-03*
