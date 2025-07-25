# Torrent Microservice

This service handles offloading aged BSV blocks to a torrent overlay network with incentives and merkle proof support.

## Config

Add to tests/config.toml:

[torrent_service]
piece_size = 33554432  # 32MB
aged_threshold_months = 60
# etc.

## Submodules

- chunker.rs: Torrent creation.
- tracker.rs: Peer management.
- proof_server.rs: Merkle proofs RPC.
- incentives.rs: BSV rewards.
- aging.rs: Pruning triggers.
- utils.rs: Helpers.
