// torrent_service/src/chunker.rs
use bip_metainfo::MetainfoBuilder;
use crate::utils::{Config, ServiceError};
use sv::block::Block;
use hex;

pub struct Chunker {
    piece_size: usize,
}

impl Chunker {
    pub fn new(config: &Config) -> Self {
        Self { piece_size: config.piece_size }
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

        let chunks = data.chunks(self.piece_size);
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
