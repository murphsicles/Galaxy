# Alert Service

The Alert Service manages alert notifications for Bitcoin SV (BSV) node operations, optimized from the ground-up for ultra high-throughput processing to support robust monitoring across services.

## Features

- **Alert Processing**: Processes and logs alerts with `SendAlert` RPC.
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

- **Rust Crates**: `tonic`, `prometheus`, `tracing`, `governor`, `tokio`.
- **Internal Services**: `auth_service` (`:50060`).
- **Proto Files**: `alert.proto`, `auth.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `alert_requests_total`: Total alert requests.
- `alert_latency_ms`: Average alert processing latency (ms).
- `alert_alert_count`: Total alerts processed.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).
- `index_throughput`: Indexed items per second (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: No direct block size limit enforced; relies on `block_service` (temporary 32GB limit) and `consensus_service`.
- **OP_RETURN**: No direct OP_RETURN limit enforced; relies on `transaction_service` and `consensus_service` (4.3GB limit).
- **Transaction Size**: No direct transaction size limit enforced; relies on `consensus_service` (temporary 32GB limit).
- The service supports BSV’s massive transaction and block sizes by handling alerts from other services.

## Usage

1. Start the service:
   ```bash
   cargo run --bin alert_service
   ```
   Listens on `[::1]:50061`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"event_type": "error", "message": "Test alert", "severity": 2}' [::1]:50061 alert.Alert/SendAlert
   ```

## Testing

- Use `tests/config.toml` to configure test parameters.
- Run integration tests with `cargo test` to verify alert processing.

## Notes

- Alerts are logged and processed for failures across services (e.g., validation, indexing, broadcast).
- Designed for high-throughput, supporting BSV’s massive transaction and block sizes through integration with other services.

---
*Last updated: 2025-07-03*
