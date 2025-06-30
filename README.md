# 🚀 Galaxy
**The Ultra High-Performance BSV Node**

![Rust](https://img.shields.io/badge/Rust-1.80+-orange?logo=rust)
![Build Status](https://github.com/murphsicles/Galaxy/actions/workflows/ci.yml/badge.svg)
![Dependencies](https://img.shields.io/badge/dependencies-up%20to%20date-green)
![License](https://img.shields.io/badge/license-Open%20BSV-blue)

**Galaxy** is an ultra high-performance Bitcoin SV (BSV) node built in Rust, designed to unboundedly scale to asynchronously parallel process over **100,000,000 transactions per second (TPS)** per shard. It leverages a microservices architecture with gRPC for ultra-fast, asynchronous communication, Tiger Beetle DB for scalable UTXO storage, and Rust SV for BSV-specific libraries to power OP_RETURN data, private blockchain overlays, and Simplified Payment Verification (SPV) proofs.

## 🌟 Features

- **High Throughput**: Targets over 100,000,000 TPS with async gRPC and batch processing.
- **BSV-Specific**:
  - Supports unbounded block creation.
  - Handles OP_RETURN data for enterprise applications.
  - Manages private blockchain overlays.
  - Provides SPV proofs for lightweight clients.
- **Microservices Architecture**:
  - `network_service`: P2P networking with peer pool management.
  - `transaction_service`: Transaction validation and queuing.
  - `block_service`: Block validation and assembly.
  - `storage_service`: UTXO management with Tiger Beetle.
  - `consensus_service`: Enforces BSV consensus rules.
  - `overlay_service`: Manages private blockchains.
  - `validation_service`: Generates/verifies SPV proofs.
  - `mining_service`: Supports block mining.
- **Performance Optimizations**
  - Asynchronous gRPC calls for non-blocking operations.
  - Batching for transactions, blocks, and UTXOs to reduce network overhead.
  - Lean Protocol Buffer messages for fast serialization.
  - Transaction queuing for high-throughput processing.
  - Scalable UTXO storage with Tiger Beetle DB.

## 🛠️ Setup

### Prerequisites
- Rust 1.80+ (stable)
- Cargo
- Tiger Beetle server (for `storage_service`, see [Tiger Beetle](https://github.com/tigerbeetle/tigerbeetle))
- `grpcurl` for testing

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

### Tiger Beetle Setup
For `storage_service`, start a Tiger Beetle server:
```bash
tigerbeetle start --cluster=0 --replica=0
```

## 🧪 Testing

Test services using `grpcurl`. Ensure all services are running on their respective ports:
- `network_service`: `localhost:50051`
- `transaction_service`: `localhost:50052`
- `storage_service`: `localhost:50053`
- `block_service`: `localhost:50054`
- `consensus_service`: `localhost:50055`
- `overlay_service`: `localhost:50056`
- `validation_service`: `localhost:50057`
- `mining_service`: `localhost:50058`

### Testnet Integration
Galaxy is configured to connect to BSV testnet nodes. See `tests/config.toml` for settings:
- Testnet nodes for `network_service`
- Tiger Beetle server address for `storage_service`
- Test cases for all services

Run the full pipeline test:
```bash
cd tests
chmod +x run_tests.sh
./run_tests.sh
```

See individual service READMEs for detailed test commands.

## 🔄 CI/CD

Galaxy uses GitHub Actions for continuous integration and deployment:
- **Build**: Compiles the project in release mode.
- **Formatting**: Ensures code adheres to `rustfmt` standards.
- **Linting**: Runs `clippy` for code quality.
- **Dependency Checks**: Validates dependencies with `cargo outdated`.

Check the [CI workflow](.github/workflows/ci.yml) for details.

## 📈 Performance Highlights

Galaxy is optimized for ultra-high performance:
- **Asynchronous gRPC**: Non-blocking calls for maximum concurrency.
- **Batching**: Reduces network overhead for transactions, blocks, and UTXOs.
- **Tiger Beetle DB**: Scalable UTXO storage for BSV’s future dataset.
- **Transaction Queuing**: Handles very high transaction volumes efficiently.
- **Lean Messages**: Minimizes serialization overhead.

These optimizations position Galaxy to surpass competitors like Teranode, targeting over 100,000,000 TPS per shard.

## 📚 Project Structure

The project is organized as a Rust workspace with the following structure:

| Directory            | Description                          |
|----------------------|--------------------------------------|
| `network_service/`   | P2P networking for BSV nodes         |
| `transaction_service/`| Transaction validation and queuing   |
| `block_service/`     | Block validation and assembly        |
| `storage_service/`   | UTXO storage with Tiger Beetle       |
| `consensus_service/` | BSV consensus rule enforcement       |
| `overlay_service/`   | Private blockchain overlays          |
| `validation_service/`| SPV proof generation and verification|
| `mining_service/`    | Block mining support                 |
| `shared/`            | Shared utilities and types           |
| `protos/`            | gRPC proto files for services        |
| `tests/`             | Test configuration and scripts       |
| `Cargo.toml`         | Workspace configuration              |

## 🤝 Contributing

Contributions are welcome! Please open an issue or pull request on GitHub.

## 📝 License

Licensed under the Open BSV License. See [LICENSE](LICENSE) for details.

## 📬 Contact

For questions, reach out to **murphsicles** via GitHub issues.

🌌 **Galaxy: Powering the future of BSV with unmatched performance!**
