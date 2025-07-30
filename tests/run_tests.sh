#!/bin/bash

# Ensure Tiger Beetle is running
tigerbeetle start --cluster=0 --replica=0 &

# Start all services in background
cd network_service && cargo run &
cd ../transaction_service && cargo run &
cd ../block_service && cargo run &
cd ../storage_service && cargo run &
cd ../consensus_service && cargo run &
cd ../overlay_service && cargo run &
cd ../validation_service && cargo run &
cd ../mining_service && cargo run &
cd ../torrent_service && cargo run &
cd ../merchant_service && cargo run &

# Wait for services to start
sleep 5

# Run tests using grpcurl
echo "Testing network_service: Ping"
grpcurl -plaintext -d '{"message": "Hello"}' localhost:50051 network.Network/Ping

echo "Testing network_service: DiscoverPeers"
grpcurl -plaintext -d '{"node_id": "galaxy_node"}' localhost:50051 network.Network/DiscoverPeers

echo "Testing transaction_service: ValidateTransaction"
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50052 transaction.Transaction/ValidateTransaction

echo "Testing block_service: ValidateBlock"
grpcurl -plaintext -d '{"block_hex": "01000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000ffffffff"}' localhost:50054 block.Block/ValidateBlock

echo "Testing storage_service: AddUtxo"
grpcurl -plaintext -d '{"txid": "abc123", "vout": 0, "script_pubkey": "76a91488a5e4a4e6c4a4e0c7b0b4a4e4a4e4a4e4a4e4a488ac", "amount": 1000000}' localhost:50053 storage.Storage/AddUtxo

echo "Testing consensus_service: ValidateTransactionConsensus"
grpcurl -plaintext -d '{"tx_hex": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}' localhost:50055 consensus.Consensus/ValidateTransactionConsensus

echo "Testing overlay_service: CreateOverlay"
grpcurl -plaintext -d '{"overlay_id": "test_overlay"}' localhost:50056 overlay.Overlay/CreateOverlay

echo "Testing validation_service: GenerateSPVProof"
grpcurl -plaintext -d '{"txid": "abc123"}' localhost:50057 validation.Validation/GenerateSPVProof

echo "Testing validation_service: StreamSPVProofs"
# Note: Requires a streaming-capable client
# grpcurl -plaintext -d '{"txid": "abc123"}' localhost:50057 validation.Validation/StreamSPVProofs

echo "Testing mining_service: GetMiningWork"
grpcurl -plaintext -d '{"miner_id": "miner1"}' localhost:50058 mining.Mining/GetMiningWork

# Run torrent_service test
echo "Running torrent_service unit tests..."
cargo test --test torrent_service -- --nocapture

# Test merchant_service (REST, use curl)
echo "Testing merchant_service: Health"
curl http://localhost:50063/v1/health

echo "Testing merchant_service: Policy"
curl http://localhost:50063/v1/policy

echo "Testing merchant_service: Submit Tx"
curl -X POST http://localhost:50063/v1/tx -H "Authorization: Bearer test_token" -H "Content-Type: application/json" -d '{"raw_tx": "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0100ffffffff0100ffffffff"}'

# Clean up
pkill -f tigerbeetle
pkill -f cargo
