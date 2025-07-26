// torrent_service/src/chunker.rs
use bip_metainfo::MetainfoBuilder;
use crate::utils::{Config, ServiceError};
use sv::block::Block;
use hex;

pub struct Chunker {
    config: Config,
}

impl Chunker {
    pub fn new(config: &Config) -> Self {
        Self { config: config.clone() }
    }

    pub async fn offload(&self, blocks: &[Block]) -> Result<bip_metainfo::Metainfo, ServiceError> {
        if blocks.is_empty() {
            return Err(ServiceError::Torrent("No blocks provided for offloading".to_string()));
        }

        let mut builder = MetainfoBuilder::new();
        let mut data = vec![];
        let mut block_hashes = vec![];

        for block in blocks {
            let serialized = block
                .serialize()
                .map_err(|e| ServiceError::Torrent(format!("Block serialization error: {}", e)))?;
            data.extend_from_slice(&serialized);
            block_hashes.push(hex::encode(block.header.hash()));
        }

        // Determine chunk size dynamically if enabled
        let piece_size = if self.config.dynamic_chunk_size.unwrap_or(false) {
            let total_size = data.len() as u64;
            let tps = blocks.iter().map(|b| b.transactions.len() as u64).sum::<u64>();
            // Adjust chunk size based on block size or TPS
            if total_size > 100 * 1024 * 1024 || tps > 1000000 {
                // Smaller chunks (8MB) for large or high-TPS blocks
                8 * 1024 * 1024
            } else {
                // Default 32MB for smaller blocks
                self.config.piece_size
            }
        } else {
            self.config.piece_size
        };

        let chunks = data.chunks(piece_size);
        for chunk in chunks {
            builder.add_piece(chunk);
        }

        // Embed block hashes in torrent metadata for SPV reference
        builder.set_comment(&block_hashes.join(","));
        builder
            .build()
            .map_err(|e| ServiceError::Torrent(format!("Failed to build torrent: {}", e)))
    }
}
