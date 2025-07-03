# Index Service

The Index Service manages indexing of transactions and blocks for Bitcoin SV (BSV), optimized from the ground-up for ultra high-throughput processing to support efficient querying and retrieval.

## Features

- **Transaction Indexing**: Indexes transactions with `IndexTransaction` RPC.
- **Block Indexing**: Indexes blocks with `IndexBlock` RPC.
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

- **Rust Crates**: `tonic`, `prometheus`, `sled`, `hex`, `tracing`, `governor`, `tokio`.
- **Internal Services**: `auth_service` (`:50060`), `alert_service` (`:50061`).
- **Proto Files**: `index.proto`, `auth.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `index_requests_total`: Total index requests.
- `index_latency_ms`: Average index request latency (ms).
- `index_alert_count`: Total alerts sent.
- `index_index_throughput`: Indexed transactions or blocks per second.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: No direct block size limit enforced; relies on `block_service` (temporary 32GB limit).
- **OP_RETURN**: No direct OP_RETURN limit enforced; relies on `transaction_service` and `consensus_service` (4.3GB limit).
- **Transaction Size**: No direct transaction size limit enforced; relies on `consensus_service` (temporary 32GB limit).
- The service supports BSV’s massive block and transaction sizes through integration with other services.

## Usage

1. Start the service:
   ```bash
   cargo run --bin index_service
   ```
   Listens on `[::1]:50062`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"tx_hex": "01000000..."}' [::1]:50062 index.Index/IndexTransaction
   grpcurl -plaintext -d '{"block_hex": "01000000..."}' [::1]:50062 index.Index/IndexBlock
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify transaction and block indexing.

## Notes

- Uses `sled` for persistent storage of transaction and block indices.
- Alerts are sent via `alert_service` for failures (e.g., indexing errors).
- Designed for high-throughput, supporting BSV’s massive block sizes (up to 32GB temporarily) and OP_RETURN data (up to 4.3GB) through validation by other services.

---
*Last updated: 2025-07-03*
