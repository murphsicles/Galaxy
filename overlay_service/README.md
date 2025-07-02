# Overlay Service

The Overlay Service manages overlay networks for Bitcoin SV (BSV), enabling custom consensus rules and transaction processing for specific use cases. It supports the creation, management, and querying of overlay blocks and transactions, optimized for high-throughput processing in line with Teranode's scalability goals ([https://bsv-blockchain.github.io/teranode/](https://bsv-blockchain.github.io/teranode/)).

## Features

- **Overlay Creation**: Create custom overlay networks with `CreateOverlay` RPC.
- **Transaction Processing**: Submit and process transactions with `SubmitOverlayTransaction`, `BatchSubmitOverlayTransaction`, and `StreamOverlayTransactions` RPCs, supporting large-scale OP_RETURN data up to 4.3GB.
- **Block Management**: Assemble and query overlay blocks with `GetOverlayBlock` and `QueryOverlayBlock` RPCs, with a temporary block size limit of 32GB.
- **Consensus Management**: Configure overlay-specific consensus rules (e.g., `restrict_op_return`) with `ManageOverlayConsensus` RPC.
- **Indexing**: Index overlay transactions with `IndexOverlayTransaction` RPC.
- **Metrics**: Monitor performance with Prometheus metrics via `GetMetrics` RPC, including `requests_total`, `avg_latency_ms`, `errors_total`, `cache_hits`, `alert_count`, and `index_throughput`.

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

- **Rust Crates**: `tonic`, `prometheus`, `sled`, `hex`, `tracing`, `governor`, `tokio`, `futures`, `async-stream`.
- **Internal Services**: `transaction_service` (`:50052`), `block_service` (`:50054`), `network_service` (`:50051`), `auth_service` (`:50060`), `index_service` (`:50062`), `alert_service` (`:50061`).
- **Proto Files**: `overlay.proto`, `transaction.proto`, `block.proto`, `network.proto`, `auth.proto`, `index.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `overlay_tx_requests_total`: Total transaction requests.
- `overlay_latency_ms`: Average transaction processing latency (ms).
- `overlay_block_count`: Total overlay blocks created.
- `overlay_alert_count`: Total alerts sent.
- `overlay_index_throughput`: Indexed transactions per second.

## Consensus Rules

- **OP_RETURN Limit**: Enforces a 4.3GB limit for OP_RETURN scripts when `restrict_op_return` is enabled, configurable via `ManageOverlayConsensus`.
- **Block Size Limit**: Temporary 32GB limit for overlay blocks, enforced in `assemble_overlay_block`.

## Usage

1. Start the service:
   ```bash
   cargo run --bin overlay_service
   ```
   Listens on `[::1]:50056`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"overlay_id": "test_overlay"}' [::1]:50056 overlay.Overlay/CreateOverlay
   grpcurl -plaintext -d '{"overlay_id": "test_overlay", "tx_hex": "01000000..."}' [::1]:50056 overlay.Overlay/SubmitOverlayTransaction
   grpcurl -plaintext -d '{"overlay_id": "test_overlay", "rule_name": "restrict_op_return", "enable": true}' [::1]:50056 overlay.Overlay/ManageOverlayConsensus
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify overlay creation, transaction submission, and block assembly.

## Notes

- The service uses `sled` for persistent storage of overlay blocks.
- Alerts are sent via `alert_service` for failures (e.g., validation, indexing, broadcast).
- Designed for high-throughput, supporting BSV’s large block sizes (up to 32GB temporarily) and massive OP_RETURN data (up to 4.3GB).

---
*Last updated: 2025-07-02*
