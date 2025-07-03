# Block Service

The Block Service handles block validation, assembly, and indexing for Bitcoin SV (BSV), optimized from the ground-up for ultra high-throughput processing.

## Features

- **Block Validation**: Validates blocks with `ValidateBlock` RPC, enforcing a temporary 32GB block size limit.
- **Block Assembly**: Assembles blocks with `AssembleBlock` RPC, supporting large blocks up to 32GB.
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

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`, `futures`.
- **Internal Services**: `transaction_service` (`:50052`), `storage_service` (`:50053`), `consensus_service` (`:50055`), `auth_service` (`:50060`), `index_service` (`:50062`), `alert_service` (`:50061`).
- **Proto Files**: `block.proto`, `transaction.proto`, `storage.proto`, `consensus.proto`, `auth.proto`, `index.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `block_requests_total`: Total block requests.
- `block_latency_ms`: Average block processing latency (ms).
- `block_alert_count`: Total alerts sent.
- `block_index_throughput`: Indexed blocks per second.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: Enforces a temporary 32GB limit in `validate_block` and `assemble_block`.
- **OP_RETURN**: Enforces a 4.3GB limit via `consensus_service` during transaction validation.
- **Transaction Size**: Enforces a 32GB limit via `consensus_service` during transaction validation.
- The service ensures compatibility with BSV’s massive block and OP_RETURN capabilities.

## Usage

1. Start the service:
   ```bash
   cargo run --bin block_service
   ```
   Listens on `[::1]:50054`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"block_hex": "01000000..."}' [::1]:50054 block.Block/ValidateBlock
   grpcurl -plaintext -d '{"tx_hexes": ["01000000...", "02000000..."]}' [::1]:50054 block.Block/AssembleBlock
   grpcurl -plaintext -d '{"block_hex": "01000000..."}' [::1]:50054 block.Block/IndexBlock
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify block validation, assembly, and indexing.

## Notes

- The service updates UTXOs via `storage_service` during block validation.
- Alerts are sent via `alert_service` for failures (e.g., block size, transaction validation, UTXO updates, indexing).
- Designed for high-throughput, supporting BSV’s massive block sizes (up to 32GB temporarily) and OP_RETURN data (up to 4.3GB) through `consensus_service` validation.

---
*Last updated: 2025-07-03*
