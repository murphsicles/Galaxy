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
        let mut builder = MetainfoBuilder::new();
        let mut data = vec![];
        for block in blocks {
            data.extend_from_slice(&block.serialize()?);
        }
        let chunks = data.chunks(self.piece_size);
        for chunk in chunks {
            builder.add_piece(chunk);
        }
        // Embed BSV block hash in metadata
        if let Some(first_block) = blocks.first() {
            builder.set_comment(&hex::encode(first_block.header.hash()));
        }
        builder.build().map_err(ServiceError::from)
    }
}
