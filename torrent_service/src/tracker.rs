// torrent_service/src/tracker.rs
use crate::utils::{Config, ServiceError};
use torrust_tracker::Tracker;
use torrust_tracker::config::Configuration as TrackerConfig;
use tokio::sync::mpsc;
use tracing::{info, error};
use bip_metainfo::Metainfo;
use std::sync::Arc;

pub struct TrackerManager {
    tracker: Tracker,
    config: Config,
    // Channel to notify service of tracker events, if needed
    event_tx: mpsc::Sender<TrackerEvent>,
}

#[derive(Debug)]
pub enum TrackerEvent {
    SeederRegistered(String, String), // peer_id, info_hash
    SeederDropped(String, String),
}

impl TrackerManager {
    pub async fn new(config: &Config) -> Self {
        let tracker_config = TrackerConfig {
            port: config.tracker_port.unwrap_or(6969),
            private: true, // Enforce private tracker mode for BSV auth
            ..TrackerConfig::default()
        };
        let tracker = Tracker::new(&tracker_config)
            .await
            .expect("Failed to initialize tracker");
        let (event_tx, _) = mpsc::channel(32); // Receiver handled by service if needed

        Self {
            tracker,
            config: config.clone(),
            event_tx,
        }
    }

    pub async fn announce(&self, torrent: &Metainfo) -> Result<(), ServiceError> {
        // Announce torrent to tracker
        self.tracker
            .add_torrent(torrent)
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to announce torrent: {}", e)))?;
        info!("Announced torrent with info_hash: {}", torrent.info_hash());
        Ok(())
    }

    pub async fn get_seeders(&self, info_hash: &str) -> Result<Vec<String>, ServiceError> {
        // Fetch list of seeders for the given info_hash
        let peers = self.tracker
            .get_peers(info_hash)
            .await
            .map_err(|e| ServiceError::Torrent(format!("Failed to fetch peers: {}", e)))?;
        let seeder_addrs: Vec<String> = peers
            .into_iter()
            .filter(|peer| peer.is_seeder)
            .map(|peer| peer.address.to_string())
            .collect();
        Ok(seeder_addrs)
    }

    pub async fn run(&self) {
        // Start tracker loop
        info!("Starting tracker on port {}", self.config.tracker_port.unwrap_or(6969));
        if let Err(e) = self.tracker.run().await {
            error!("Tracker error: {}", e);
        }
    }
}
