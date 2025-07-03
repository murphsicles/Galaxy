# Storage Service

The Storage Service manages UTXO storage and querying for Bitcoin SV (BSV), optimized from the ground-up for ultra high-throughput processing to support massive transaction volumes.

## Features

- **UTXO Querying**: Queries UTXOs with `QueryUtxo` RPC.
- **UTXO Management**: Adds and removes UTXOs with `AddUtxo`, `RemoveUtxo`, and `BatchAddUtxo` RPCs.
- **Metrics**: Monitors performance with Prometheus metrics via `GetMetrics` RPC, including `requests_total`, `avg_latency_ms`, `errors_total`, `cache_hits`, `alert_count`, and `index_throughput`.
- **TigerBeetle Integration**: Placeholder for future high-performance storage using the `tigerbeetle` crate (currently uses in-memory `HashMap`).

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

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`, `tigerbeetle`.
- **Internal Services**: `auth_service` (`:50060`), `alert_service` (`:50061`).
- **Proto Files**: `storage.proto`, `auth.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `storage_requests_total`: Total storage requests.
- `storage_latency_ms`: Average storage request latency (ms).
- `storage_alert_count`: Total alerts sent.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).
- `index_throughput`: Indexed items per second (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: No direct block size limit enforced; relies on `block_service` (temporary 32GB limit) and `consensus_service`.
- **OP_RETURN**: No direct OP_RETURN limit enforced; relies on `transaction_service` and `consensus_service` (4.3GB limit).
- **Transaction Size**: No direct transaction size limit enforced; relies on `consensus_service` (temporary 32GB limit).
- The service supports BSV’s massive transaction and block sizes through integration with other services.

## Usage

1. Start the service:
   ```bash
   cargo run --bin storage_service
   ```
   Listens on `[::1]:50053`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"txid": "abc123...", "vout": 0}' [::1]:50053 storage.Storage/QueryUtxo
   grpcurl -plaintext -d '{"txid": "abc123...", "vout": 0, "script_pubkey": "76a914...", "amount": 1000}' [::1]:50053 storage.Storage/AddUtxo
   grpcurl -plaintext -d '{"txid": "abc123...", "vout": 0}' [::1]:50053 storage.Storage/RemoveUtxo
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify UTXO querying and management.

## Notes

- Currently uses an in-memory `HashMap` for UTXO storage; `tigerbeetle` integration planned for future scalability.
- Alerts are sent via `alert_service` for failures (e.g., UTXO query, add, or remove errors).
- Designed for high-throughput, supporting BSV’s massive transaction and block sizes through validation by other services.

---
*Last updated: 2025-07-03*
