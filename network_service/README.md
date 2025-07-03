# Network Service

The Network Service handles peer-to-peer networking for Bitcoin SV (BSV), facilitating transaction and block broadcasting, peer discovery, and network communication, designed from the ground-up for ultra high-throughput processing.

## Features

- **Peer Management**: Connects to and manages BSV testnet peers with `DiscoverPeers` RPC.
- **Transaction Broadcasting**: Broadcasts transactions to peers with `BroadcastTransaction` RPC, supporting large-scale OP_RETURN data (validated by other services).
- **Block Broadcasting**: Broadcasts blocks to peers with `BroadcastBlock` RPC, supporting large blocks (validated by other services).
- **Ping**: Tests connectivity with `Ping` RPC.
- **Metrics**: Monitors performance with Prometheus metrics via `GetMetrics` RPC, including `requests_total`, `avg_latency_ms`, `errors_total`, `cache_hits`, `alert_count`, and `index_throughput`.

## Configuration

The service uses `tests/config.toml` for configuration:
- **Sharding**: Configures `shard_id` and `shard_count` for distributed processing.
- **Testnet Peers**: Lists peer nodes under `[testnet]` for peer discovery.
- **Metrics**: Enables Prometheus endpoint (`enable_prometheus`, port: `9090`) and sets alert thresholds (`alert_threshold`) and log level (`log_level`).

Example `tests/config.toml`:
```toml
[sharding]
shard_id = 0
shard_count = 4

[testnet]
nodes = ["127.0.0.1:8333", "192.168.1.100:8333"]

[metrics]
enable_prometheus = true
prometheus_port = 9090
alert_threshold = 5
log_level = "info"
```

## Dependencies

- **Rust Crates**: `tonic`, `prometheus`, `hex`, `tracing`, `governor`, `tokio`, `futures`, `async-stream`.
- **Internal Services**: `transaction_service` (`:50052`), `block_service` (`:50054`), `auth_service` (`:50060`), `alert_service` (`:50061`).
- **Proto Files**: `network.proto`, `transaction.proto`, `block.proto`, `auth.proto`, `alert.proto`, `metrics.proto`.

## Metrics

Exposed via `GetMetrics` RPC and Prometheus endpoint (`:9090`):
- `network_requests_total`: Total network requests.
- `network_latency_ms`: Average request latency (ms).
- `network_alert_count`: Total alerts sent.
- `errors_total`: Total errors (placeholder, currently 0).
- `cache_hits`: Cache hits (not applicable, set to 0).
- `index_throughput`: Indexed items per second (not applicable, set to 0).

## Consensus and Size Limits

- **Block Size**: No direct block size limit enforced; relies on `block_service` and `consensus_service` for validation (temporary 32GB limit).
- **OP_RETURN**: No direct OP_RETURN limit enforced; relies on `transaction_service` and `consensus_service` for validation (4.3GB limit).
- The service broadcasts validated transactions and blocks to peers, ensuring compatibility with BSV’s large block and OP_RETURN capabilities.

## Usage

1. Start the service:
   ```bash
   cargo run --bin network_service
   ```
   Listens on `[::1]:50051`.

2. Example gRPC calls (using `grpcurl`):
   ```bash
   grpcurl -plaintext -d '{"message": "test"}' [::1]:50051 network.Network/Ping
   grpcurl -plaintext -d '{}' [::1]:50051 network.Network/DiscoverPeers
   grpcurl -plaintext -d '{"tx_hex": "01000000..."}' [::1]:50051 network.Network/BroadcastTransaction
   grpcurl -plaintext -d '{"block_hex": "01000000..."}' [::1]:50051 network.Network/BroadcastBlock
   ```

## Testing

- Use `tests/config.toml` to configure testnet peers and test parameters.
- Run integration tests with `cargo test` to verify peer discovery, transaction, and block broadcasting.

## Notes

- The service uses TCP streams for peer communication, with version handshakes (`version`, `verack`) per BSV protocol.
- Alerts are sent via `alert_service` for failures (e.g., peer discovery, broadcast).
- Designed for high-throughput, supporting BSV’s massive block sizes and OP_RETURN data through validation by other services.

---
*Last updated: 2025-07-03*
