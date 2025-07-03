# Mining Service

The Mining Service manages block template generation and mined block submission for Bitcoin SV (BSV), optimized from the ground-up for ultra high-throughput processing to support massive transaction volumes.

## Features

- **Block Template Generation**: Generates mining work with `GetMiningWork` and `BatchGetMiningWork` RPCs, enforcing a temporary 32GB block size limit.
- **Block Submission**: Submits mined blocks with `SubmitMinedBlock` RPC, validating proof-of-work and block size.
- **Work Streaming**: Streams mining work with `StreamMiningWork` RPC.
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

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`, `futures`, `async-stream`.
- **Internal Services**: `block_service` (`:50054`), `network_service` (`:50051`), `auth_service` (`:50060`), `alert_service` (`:50061`).
- **Proto Files**: `mining.proto`, `block.proto`, `network.proto`, `auth.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `mining_work_requests_total`: Total mining work requests.
- `mining_latency_ms`: Average work generation latency (ms).
- `mining_blocks_submitted`: Total blocks submitted.
- `mining_alert_count`: Total alerts sent.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).
- `index_throughput`: Indexed items per second (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: Enforces a temporary 32GB limit in `generate_block_template` and `submit_mined_block`.
- **OP_RETURN**: Enforces a 4.3GB limit via `consensus_service` during transaction validation.
- **Transaction Size**: Enforces a 32GB limit via `consensus_service` during transaction validation.
- The service supports BSV’s massive block and transaction sizes through integration with other services.

## Usage

1. Start the service:
   ```bash
   cargo run --bin mining_service
   ```
   Listens on `[::1]:50058`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"miner_id": "miner1"}' [::1]:50058 mining.Mining/GetMiningWork
   grpcurl -plaintext -d '{"block_hex": "01000000..."}' [::1]:50058 mining.Mining/SubmitMinedBlock
   grpcurl -plaintext -d '{"miner_ids": ["miner1", "miner2"]}' [::1]:50058 mining.Mining/BatchGetMiningWork
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify block template generation and block submission.

## Notes

- The service validates proof-of-work and broadcasts blocks via `network_service`.
- Alerts are sent via `alert_service` for failures (e.g., block size, proof-of-work, broadcast).
- Designed for high-throughput, supporting BSV’s massive block sizes (up to 32GB temporarily) and OP_RETURN data (up to 4.3GB) through `consensus_service` validation.

---
*Last updated: 2025-07-03*
