// torrent_service/src/lib.rs
pub mod service;
pub use service::TorrentService;

pub mod chunker;
pub mod tracker;
pub mod proof_server;
pub mod incentives;
pub mod aging;
pub mod utils;
