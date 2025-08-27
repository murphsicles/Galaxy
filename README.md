# 🚀 Galaxy
**The Ultra High-Performance BSV Node**

***Note: Galaxy is NOT concensus ready. For testing purposes only until release v1.0**

![Rust](https://img.shields.io/badge/Rust-1.88+-orange?logo=rust)
![Build Status](https://github.com/murphsicles/Galaxy/actions/workflows/ci.yml/badge.svg)
![Dependencies](https://deps.rs/repo/github/murphsicles/Galaxy/status.svg)
![License](https://img.shields.io/badge/license-Open%20BSV-blue)

**Galaxy** is an ultra high-performance Bitcoin SV (BSV) node built in Rust, designed to unboundedly scale to asynchronously parallel process over **100,000,000 transactions per second (TPS)** per shard. It leverages a microservices architecture with a novel asyncTCP implementation written from the ground-up for ultra-fast, asynchronous communication. Featuring a bespoke Tiger Beetle DB for scalable UTXO storage, and Rust SV for BSV-specific libraries to power OP_RETURN data, private blockchain overlays, and Simplified Payment Verification (SPV) proofs.

## 🌟 Features

- **High Throughput**: Targets over 100,000,000 TPS with async TCP, batching, and sharding.
- **BSV-Specific**:
  - Supports unbounded block creation.
  - Handles OP_RETURN data for enterprise applications.
  - Manages private blockchain overlays with persistent storage (sled).
  - Provides SPV proofs with caching and streaming.
  - Supports mining with dynamic difficulty and transaction selection.
  - Decentralized data distribution via torrent-like protocol with sybil resistance.
- **Security**:
  - Token-based authentication with JWTs.
  - Role-based access control (RBAC) for miners and clients.
  - Rate limiting to prevent abuse (1000 req/s per service).
  - Sybil resistance for decentralized data sharing.
- **Microservices Architecture**:
  - `network_service`: P2P networking with peer pool management.
  - `transaction_service`: Transaction validation, queuing, and indexing.
  - `block_service`: Block validation, assembly, and indexing with merkle root computation.
  - `storage_service`: UTXO management with Tiger Beetle integration.
  - `consensus_service`: Enforces BSV consensus rules.
  - `overlay_service`: Manages private blockchains with streaming, storage, and indexing.
  - `validation_service`: Generates/verifies SPV proofs with caching and streaming.
  - `mining_service`: Supports block mining with streaming and transaction selection.
  - `auth_service`: Handles authentication and authorization.
  - `alert_service`: Monitors network health and sends notifications.
  - `index_service`: Indexes transactions and blocks for efficient querying.
  - `api_service`: Public-facing API gateway for transaction submission and block queries.
  - `torrent_service`: Decentralized data distribution for aged blocks and SPV proofs, with sybil resistance and dynamic chunk sizing.
  - `merchant_service`: Merchant API for transaction submission, broadcasting, status tracking, and callbacks with REST endpoints.
- **Performance Optimizations**:
  - Asynchronous TCP communication with `bincode` serialization.
  - Batching for transactions, blocks, and UTXOs.
  - Lean message structures with 4GB buffer hints for large blocks.
  - Transaction queuing and SPV proof caching.
  - Scalable UTXO storage with Tiger Beetle DB.
  - Dynamic chunk sizing for efficient data transfer in torrent-like protocol.
  - Prometheus metrics for performance monitoring.
  - Structured logging with tracing for observability.
  - Sharding for distributed processing.
- **Monitoring**:
  - Alert notifications for critical events (e.g., consensus violations, performance issues).
  - Transaction and block indexing for efficient querying.
  - Prometheus metrics for TPS, latency, and errors across all services.

## 🛠️ Setup

### Prerequisites
- Rust 1.88+ (stable)
- Cargo
- Tiger Beetle server (for `storage_service`, see [Tiger Beetle](https://github.com/tigerbeetle/tigerbeetle))
- `sled` for overlay and index storage
- `prometheus`, `governor`, `jsonwebtoken`, and `tracing` for metrics, rate limiting, auth, and logging

### Installation
```bash
git clone https://github.com/murphsicles/Galaxy.git
cd Galaxy
cargo build
```

### Running Services
Start each service in a separate terminal:
```bash
cd network_service
cargo run
```
```bash
cd transaction_service
cargo run
```
```bash
cd block_service
cargo run
```
```bash
cd storage_service
cargo run
```
```bash
cd consensus_service
cargo run
```
```bash
cd overlay_service
cargo run
```
```bash
cd validation_service
cargo run
```
```bash
cd mining_service
cargo run
```
```bash
cd auth_service
cargo run
```
```bash
cd alert_service
cargo run
```
```bash
cd index_service
cargo run
```
```bash
cd api_service
cargo run
```
```bash
cd torrent_service
cargo run
```
```bash
cd merchant_service
cargo run
```

### Tiger Beetle Setup
For `storage_service`, start a Tiger Beetle server:
```bash
tigerbeetle start --cluster=0 --replica=0
```

### Authentication Setup
Configure a secure JWT secret key in `tests/config.toml` under `[auth]`. Generate tokens with roles (`client`, `miner`) for testing.

## 🧪 Testing

Test services using a TCP client or custom scripts with `bincode` serialization, including JWT tokens for authentication. Ensure all services are running on their respective ports:
- `api_service`: `localhost:50050`
- `network_service`: `localhost:50051`
- `transaction_service`: `localhost:50052`
- `storage_service`: `localhost:50053`
- `block_service`: `localhost:50054`
- `consensus_service`: `localhost:50055`
- `overlay_service`: `localhost:50056`
- `validation_service`: `localhost:50057`
- `mining_service`: `localhost:50058`
- `index_service`: `localhost:50059`
- `auth_service`: `localhost:50060`
- `alert_service`: `localhost:50061`
- `torrent_service`: `localhost:50062`
- `merchant_service`: `localhost:50063`

### Testnet Integration
Galaxy is configured to connect to BSV testnet nodes. See `tests/config.toml` for settings:
- Testnet nodes for `network_service`.
- Tiger Beetle server address for `storage_service`.
- Sharding parameters and node mappings.
- Authentication settings.
- Alert and indexing configurations.
- Overlay consensus rules (e.g., `restrict_op_return`).
- Torrent service configurations for sybil resistance and chunk sizing.
- Test cases for all services, including `torrent_service` with mocked dependencies.

Run the full pipeline test:
```bash
cd tests
chmod +x run_tests.sh
./run_tests.sh
```

### Metrics
Monitor performance using the `GetMetrics` endpoint on each service (e.g., `localhost:50057` for `validation_service`, dynamic port for `torrent_service` in tests):
```bash
# Example TCP client or custom script needed for bincode+tokio communication
# Use a tool like netcat or a custom Rust client to send GetMetrics requests
```
Metrics include:
- `*_requests_total`: Total requests per service (e.g., `block_requests_total`, `torrent_requests_total`).
- `*_latency_ms`: Average request latency (ms).
- `*_alert_count`: Total alerts sent.
- `*_index_throughput`: Indexed items per second (where applicable).
- `*_cache_hits`: Cache hits (e.g., `validation_cache_hits`).
- `errors_total`: Total errors (placeholder, currently 0).

### Alerts
Subscribe to alerts using the `alert_service` with a TCP client:
```bash
# Use a custom Rust client or TCP tool to send AlertRequest with bincode serialization
```

### Logging
View logs with the `tracing` crate at the `INFO` level (configurable in `tests/config.toml` under `[metrics][log_level]`).

See individual service READMEs for detailed test commands, including `torrent_service/README.md` for torrent-specific tests.

## 🔄 CI/CD

Galaxy uses GitHub Actions for continuous integration and deployment:
- **Build**: Compiles the project in release mode.
- **Formatting**: Ensures code adheres to `rustfmt` standards.
- **Linting**: Runs `clippy` for code quality.
- **Dependency Checks**: Validates dependencies with `cargo outdated`.
- **Tests**: Runs unit and integration tests, starting dependent services, including `torrent_service` with mocked block, overlay, validation, transaction, auth, alert, storage, and proof services.

Check the [CI workflow](.github/workflows/ci.yml) for details.

## 📈 Performance Highlights

Galaxy is optimized for ultra-high performance:
- **Asynchronous TCP**: Non-blocking calls with `bincode` serialization.
- **Batching**: Reduces network overhead for transactions, blocks, and UTXOs.
- **Tiger Beetle DB**: Scalable UTXO storage for BSV’s future dataset.
- **Transaction Queuing**: Handles high transaction volumes efficiently.
- **SPV Proof Caching**: Optimizes proof generation with LRU cache.
- **Streaming**: Supports continuous transaction, proof, and mining work processing.
- **Decentralized Data Distribution**: `torrent_service` enables efficient sharing of aged blocks and SPV proofs with dynamic chunk sizing.
- **Sybil Resistance**: `torrent_service` implements reputation-based access control to prevent sybil attacks.
- **Metrics**: Monitors TPS, latency, and errors with Prometheus.
- **Logging**: Structured logging with `tracing` for observability.
- **Sharding**: Distributed processing with shard-aware logic.
- **Security**: JWT authentication and RBAC for secure access.
- **Rate Limiting**: Prevents endpoint abuse with 1000 req/s limit.
- **Alerting**: Real-time notifications for network health.
- **Indexing**: Efficient querying for transactions and blocks.
- **Merchant API**: RESTful endpoints for high-volume tx processing with async delegation to other services.

## 📚 Project Structure

| Directory            | Description                          |
|----------------------|--------------------------------------|
| `network_service/`   | P2P networking for BSV nodes         |
| `transaction_service/`| Transaction validation, queuing, indexing   |
| `block_service/`     | Block validation, assembly, indexing        |
| `storage_service/`   | UTXO storage with Tiger Beetle       |
| `consensus_service/` | BSV consensus rule enforcement       |
| `overlay_service/`   | Private blockchain overlays          |
| `validation_service/`| SPV proof generation and verification|
| `mining_service/`    | Block mining support                 |
| `auth_service/`      | Authentication and authorization      |
| `alert_service/`     | Network health monitoring and notifications |
| `index_service/`     | Transaction and block indexing       |
| `api_service/`       | Public-facing API gateway for transaction submission and block queries |
| `torrent_service/`   | Decentralized data distribution for aged blocks and SPV proofs with sybil resistance |
| `merchant_service/`  | Merchant API for tx submission, status, and proofs |
| `shared/`            | Shared utilities (ShardManager)      |
| `tests/`             | Test configuration and scripts       |
| `Cargo.toml`         | Workspace configuration              |

## 🤝 Contributing

Contributions are welcome! Please open an issue or pull request on GitHub.

## 📝 License

Licensed under the Open BSV License. See [LICENSE](LICENSE) for details.

## 📬 Contact

For questions, reach out to **murphsicles** via GitHub issues.

🌌 **Galaxy: Powering the future of BSV with unmatched performance!**
