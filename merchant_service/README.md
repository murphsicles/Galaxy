# Merchant Service

The Galaxy Merchant Service is a transaction pre-processor for merchant payment over AOI
This service provides a REST API for tx lifecycle, delegates to other Galaxy services via TCP/bincode.

## Features
- Endpoints: /v1/policy, /v1/health, /v1/tx, /v1/txs, /v1/txs/chain, /v1/tx/{txid}.
- Async, rate-limited, authenticated.
- Status tracking, callbacks, proofs.

## Running
cargo run

## Testing
curl -X POST http://localhost:50063/v1/tx -H "Authorization: Bearer <jwt>" -d '{"raw_tx": "hex"}'

## Metrics
Prometheus: requests_total, latency_ms, errors_total, alert_count.
