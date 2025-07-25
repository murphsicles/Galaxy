# Torrent Microservice 🚀

This service implements a torrent overlay network on top of Bitcoin SV (BSV) to offload aged block data, reducing storage burdens for miners while maintaining data availability through a decentralized, incentivized system. It integrates seamlessly with Galaxy's microservices architecture, using Rust for asynchronous operations and compatibility with existing torrent clients like Vuze.

## Overview 📄

The Torrent Microservice solves the "aged transaction problem" in BSV by allowing miners to prune old blocks (default: >5 years) and reference them via torrents. Key premises:
- **Pruning with Availability**: Miners offload blocks to torrents, storing references in the Overlay Service.
- **SPV-Friendly**: Light clients can request merkle proofs without downloading full chunks.
- **Incentives**: BSV micropayments reward seeders (100,000 sat stake, 100 sat/MB bulk, 100 sat/proof base + bonuses).
- **Gamification**: Bonuses for fast delivery (<500ms: 10 sat) and rare blocks (<3 seeders: 50 sat).
- **Compatibility**: Standard torrents (32MB chunks) for Vuze; custom RPC for proofs.

The service is fully asynchronous, using Tokio and bincode for inter-service communication, matching Galaxy's design.

## Features ✨

- **Aging Threshold**: Configurable pruning by time (months) or block height (default: 60 months). ⚙️
- **Torrent Offloading**: Chunk blocks into 32MB pieces, generate .torrent files with BSV hash metadata. 📦
- **Seeder Authentication**: BSV wallet signatures required for registration (via tracker). 🔒
- **Merkle Proof Serving**: Hybrid approach: RPC for lightweight proofs, torrents for bulk data. 📜
- **Incentive Structure**: Micropayments for seeding/proofs, staking/slashing for behavior. 💰
- **Validation Integration**: Proofs validated via Validation Service; transactions via Transaction Service. ✅
- **Storage/Overlay Integration**: Block fetching from Storage, references in Overlay. 🗄️
- **Metrics & Alerts**: Prometheus metrics and alerts for monitoring. 📊
- **Testing**: Integration tests for end-to-end flow and bonus calculations. 🧪

## Configuration ⚙️

Add to `tests/config.toml` under `[torrent_service]`:
```toml
[torrent_service]
piece_size = 33554432 # 32MB
aged_threshold_months = 60 # Default months (or aged_threshold_blocks)
stake_amount = 100000 # sat
proof_reward_base = 100 # sat
proof_bonus_speed = 10 # sat (<500ms)
proof_bonus_rare = 50 # sat (<3 seeders)
bulk_reward_per_mb = 100 # sat
tracker_port = 6969
proof_rpc_port = 50063
auth_token = "your_auth_token" # For inter-service authentication
```
## Submodules 🛠️

- **chunker.rs**: Handles block chunking and .torrent generation with `bip_metainfo`.
- **tracker.rs**: Manages torrust-tracker for peer discovery, with BSV signature auth for seeders.
- **proof_server.rs**: RPC server for merkle proofs, fetching from swarm or local storage.
- **incentives.rs**: BSV micropayments for rewards/stakes, with bonus calculations.
- **aging.rs**: Periodic detection of aged blocks, triggering offloads.
- **utils.rs**: Errors, config structs, and enums (e.g., AgedThreshold).

## Integration 🔗

- **Block Service (50054)**: Fetches aged blocks.
- **Storage Service (50053)**: Queries blocks by timestamp/height.
- **Overlay Service (50056)**: Stores torrent references.
- **Validation Service (50057)**: Validates proofs/transactions.
- **Transaction Service (50052)**: Broadcasts incentive TXs.
- **Auth/Alert Services (50060/50061)**: Authentication and alerts.

Start with `cargo run --package torrent_service`.

## Testing 🧪

Run `./tests/run_tests.sh` to start services and test the torrent overlay flow, including proof retrieval, incentives, and bonuses.

## Future Improvements 🚧

- Dynamic chunk sizes based on block TPS.
- Full UTXO integration for TX inputs.
- Enhanced sybil resistance in tracker.

Contributions welcome! 🌟
